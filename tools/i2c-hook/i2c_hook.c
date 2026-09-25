/*
 * i2c_hook.c - logging hooks injected into GPU Tweak III processes (32-bit).
 *
 * Once loaded into a process (see inject.c), a worker thread waits for the
 * real nvapi.dll and hooks the code of its I2C functions, so every call is
 * logged no matter how or when the application resolved them. It also logs
 * DeviceIoControl calls and CreateFile opens of \\.\ devices, which covers
 * ASUS's EIO kernel-driver path.
 *
 * Hooks only observe: arguments and results pass through unchanged, and this
 * DLL never issues I2C transactions or IOCTLs of its own.
 *
 * Log files: C:\ProgramData\gt-i2c-hook\<process>-<pid>.log
 */

#define _CRT_SECURE_NO_WARNINGS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include "MinHook.h"

#define LOG_DIR L"C:\\ProgramData\\gt-i2c-hook"
#define MAX_LOGGED_BYTES 256
#define MAX_IOCTL_BYTES 64

typedef unsigned int NvU32;
typedef unsigned char NvU8;
typedef int NvAPI_Status;
typedef void *NvPhysicalGpuHandle;

/* NV_I2C_INFO, versions 1-3 share this prefix layout (from nvapi.h). */
typedef struct {
    NvU32 version;          /* sizeof(struct) | (ver << 16) */
    NvU32 displayMask;
    NvU8 bIsDDCPort;
    NvU8 i2cDevAddress;     /* 8-bit form: 7-bit address << 1 */
    NvU8 *pbI2cRegAddress;
    NvU32 regAddrSize;
    NvU8 *pbData;
    NvU32 cbSize;
    NvU32 i2cSpeed;         /* deprecated */
    NvU32 i2cSpeedKhz;      /* v2+ */
    NvU8 portId;            /* v3+ */
    NvU32 bIsPortIdSet;     /* v3+ */
} NV_I2C_INFO;

typedef void *(__cdecl *QueryInterface_t)(NvU32 id);
typedef NvAPI_Status (__cdecl *I2C_t)(NvPhysicalGpuHandle, NV_I2C_INFO *);
typedef NvAPI_Status (__cdecl *I2CEx_t)(NvPhysicalGpuHandle, NV_I2C_INFO *, NvU32 *);
typedef BOOL (WINAPI *DeviceIoControl_t)(HANDLE, DWORD, LPVOID, DWORD, LPVOID, DWORD, LPDWORD, LPOVERLAPPED);
typedef HANDLE (WINAPI *CreateFileW_t)(LPCWSTR, DWORD, DWORD, LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE);
typedef HANDLE (WINAPI *CreateFileA_t)(LPCSTR, DWORD, DWORD, LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE);

#define ID_I2C_READ     0x2FDE12C6u
#define ID_I2C_WRITE    0xE812EB07u
#define ID_I2C_READ_EX  0x4D7B0709u
#define ID_I2C_WRITE_EX 0x283AC65Au

static CRITICAL_SECTION g_log_lock;
static FILE *g_log;
static DWORD g_tls_busy = TLS_OUT_OF_INDEXES;

static I2C_t g_real_i2c_read, g_real_i2c_write;
static I2CEx_t g_real_i2c_read_ex, g_real_i2c_write_ex;
static DeviceIoControl_t g_real_ioctl;
static CreateFileW_t g_real_create_w;
static CreateFileA_t g_real_create_a;

static void log_open(void)
{
    wchar_t exe_path[MAX_PATH], log_path[MAX_PATH];
    const wchar_t *exe_name;

    CreateDirectoryW(LOG_DIR, NULL);
    GetModuleFileNameW(NULL, exe_path, MAX_PATH);
    exe_name = wcsrchr(exe_path, L'\\');
    exe_name = exe_name ? exe_name + 1 : exe_path;
    _snwprintf(log_path, MAX_PATH, LOG_DIR L"\\%s-%lu.log", exe_name, GetCurrentProcessId());
    log_path[MAX_PATH - 1] = 0;
    g_log = _wfopen(log_path, L"a");
}

static void log_line(const char *fmt, ...)
{
    SYSTEMTIME t;
    va_list ap;

    if (!g_log)
        return;
    GetLocalTime(&t);
    EnterCriticalSection(&g_log_lock);
    fprintf(g_log, "%02u:%02u:%02u.%03u [tid %lu] ",
            t.wHour, t.wMinute, t.wSecond, t.wMilliseconds, GetCurrentThreadId());
    va_start(ap, fmt);
    vfprintf(g_log, fmt, ap);
    va_end(ap);
    fputc('\n', g_log);
    fflush(g_log);
    LeaveCriticalSection(&g_log_lock);
}

/* Guards against logging our own file I/O and against re-entry from nested hooked calls. */
static BOOL enter_hook(void)
{
    if (g_tls_busy == TLS_OUT_OF_INDEXES || TlsGetValue(g_tls_busy))
        return FALSE;
    TlsSetValue(g_tls_busy, (LPVOID)1);
    return TRUE;
}

static void leave_hook(void)
{
    TlsSetValue(g_tls_busy, NULL);
}

static void hex(char *out, size_t out_size, const void *buf, DWORD n, DWORD limit)
{
    const NvU8 *p = buf;
    size_t pos = 0;
    DWORD i, shown = n > limit ? limit : n;

    out[0] = 0;
    if (!p) {
        snprintf(out, out_size, "(null)");
        return;
    }
    for (i = 0; i < shown && pos + 4 < out_size; i++)
        pos += snprintf(out + pos, out_size - pos, "%s%02X", i ? " " : "", p[i]);
    if (shown < n && pos + 5 < out_size)
        snprintf(out + pos, out_size - pos, " ...");
}

/* result is "" when logging a write before it is issued, or " -> status N" after a call. */
static void log_i2c(const char *op, NvPhysicalGpuHandle gpu, const NV_I2C_INFO *info, const char *result)
{
    char reg[64], data[MAX_LOGGED_BYTES * 3 + 8];
    NvU32 ver, size;

    if (!info) {
        log_line("%s gpu=%p info=(null)%s", op, gpu, result);
        return;
    }
    ver = info->version >> 16;
    size = info->version & 0xFFFF;
    hex(reg, sizeof reg, info->pbI2cRegAddress, info->regAddrSize, 16);
    hex(data, sizeof data, info->pbData, info->cbSize, MAX_LOGGED_BYTES);

    if (ver >= 3 && size >= sizeof(NV_I2C_INFO))
        log_line("%s gpu=%p v%u port=%u portSet=%u ddc=%u addr=0x%02X(7bit 0x%02X) disp=0x%X speedKhz=%u "
                 "reg[%u]=%s len=%u data=%s%s",
                 op, gpu, ver, info->portId, info->bIsPortIdSet, info->bIsDDCPort,
                 info->i2cDevAddress, info->i2cDevAddress >> 1, info->displayMask, info->i2cSpeedKhz,
                 info->regAddrSize, reg, info->cbSize, data, result);
    else
        log_line("%s gpu=%p v%u size=%u ddc=%u addr=0x%02X(7bit 0x%02X) disp=0x%X "
                 "reg[%u]=%s len=%u data=%s%s",
                 op, gpu, ver, size, info->bIsDDCPort,
                 info->i2cDevAddress, info->i2cDevAddress >> 1, info->displayMask,
                 info->regAddrSize, reg, info->cbSize, data, result);
}

static const char *status_text(char *buf, size_t size, NvAPI_Status s)
{
    snprintf(buf, size, " -> status %d", s);
    return buf;
}

/* Writes are logged before the call, so a crash inside the driver still leaves a record. */

static NvAPI_Status __cdecl hook_i2c_read(NvPhysicalGpuHandle gpu, NV_I2C_INFO *info)
{
    char st[32];
    NvAPI_Status s = g_real_i2c_read(gpu, info);
    log_i2c("READ    ", gpu, info, status_text(st, sizeof st, s));
    return s;
}

static NvAPI_Status __cdecl hook_i2c_write(NvPhysicalGpuHandle gpu, NV_I2C_INFO *info)
{
    NvAPI_Status s;
    log_i2c("WRITE   ", gpu, info, "");
    s = g_real_i2c_write(gpu, info);
    log_line("WRITE   -> status %d", s);
    return s;
}

static NvAPI_Status __cdecl hook_i2c_read_ex(NvPhysicalGpuHandle gpu, NV_I2C_INFO *info, NvU32 *unk)
{
    char st[32];
    NvAPI_Status s = g_real_i2c_read_ex(gpu, info, unk);
    log_i2c("READEX  ", gpu, info, status_text(st, sizeof st, s));
    return s;
}

static NvAPI_Status __cdecl hook_i2c_write_ex(NvPhysicalGpuHandle gpu, NV_I2C_INFO *info, NvU32 *unk)
{
    NvAPI_Status s;
    log_i2c("WRITEEX ", gpu, info, "");
    s = g_real_i2c_write_ex(gpu, info, unk);
    log_line("WRITEEX -> status %d", s);
    return s;
}

static BOOL WINAPI hook_ioctl(HANDLE h, DWORD code, LPVOID in, DWORD in_size, LPVOID out,
                              DWORD out_size, LPDWORD returned, LPOVERLAPPED ov)
{
    char in_hex[MAX_IOCTL_BYTES * 3 + 8], out_hex[MAX_IOCTL_BYTES * 3 + 8];
    DWORD err;
    BOOL ok;

    if (!enter_hook())
        return g_real_ioctl(h, code, in, in_size, out, out_size, returned, ov);
    hex(in_hex, sizeof in_hex, in, in_size, MAX_IOCTL_BYTES);
    ok = g_real_ioctl(h, code, in, in_size, out, out_size, returned, ov);
    err = GetLastError();
    hex(out_hex, sizeof out_hex, out, ok && returned && !ov ? *returned : 0, MAX_IOCTL_BYTES);
    log_line("IOCTL    h=%p code=0x%08lX in[%lu]=%s -> ok=%d out[%lu]=%s%s",
             h, code, in_size, in_hex, ok, ok && returned ? *returned : 0, out_hex,
             ov ? " (overlapped)" : "");
    leave_hook();
    SetLastError(err);
    return ok;
}

static HANDLE WINAPI hook_create_w(LPCWSTR name, DWORD access, DWORD share, LPSECURITY_ATTRIBUTES sa,
                                   DWORD disposition, DWORD flags, HANDLE templ)
{
    HANDLE h = g_real_create_w(name, access, share, sa, disposition, flags, templ);
    DWORD err = GetLastError();

    if (name && wcsncmp(name, L"\\\\.\\", 4) == 0 && enter_hook()) {
        log_line("OPEN     %ls -> h=%p", name, h);
        leave_hook();
    }
    SetLastError(err);
    return h;
}

static HANDLE WINAPI hook_create_a(LPCSTR name, DWORD access, DWORD share, LPSECURITY_ATTRIBUTES sa,
                                   DWORD disposition, DWORD flags, HANDLE templ)
{
    HANDLE h = g_real_create_a(name, access, share, sa, disposition, flags, templ);
    DWORD err = GetLastError();

    if (name && strncmp(name, "\\\\.\\", 4) == 0 && enter_hook()) {
        log_line("OPEN     %s -> h=%p", name, h);
        leave_hook();
    }
    SetLastError(err);
    return h;
}

static void install(const char *what, void *target, void *detour, void **original)
{
    MH_STATUS s;

    if (!target) {
        log_line("hook %s: target not found", what);
        return;
    }
    s = MH_CreateHook(target, detour, original);
    if (s == MH_OK)
        s = MH_EnableHook(target);
    log_line("hook %s at %p: %s", what, target, MH_StatusToString(s));
}

static DWORD WINAPI worker(LPVOID param)
{
    HMODULE kernelbase = GetModuleHandleW(L"kernelbase.dll");
    HMODULE nvapi;
    QueryInterface_t query;
    int waited = 0;

    (void)param;
    install("DeviceIoControl", (void *)GetProcAddress(kernelbase, "DeviceIoControl"), (void *)hook_ioctl, (void **)&g_real_ioctl);
    install("CreateFileW", (void *)GetProcAddress(kernelbase, "CreateFileW"), (void *)hook_create_w, (void **)&g_real_create_w);
    install("CreateFileA", (void *)GetProcAddress(kernelbase, "CreateFileA"), (void *)hook_create_a, (void **)&g_real_create_a);

    /* Wait for the application to load nvapi.dll itself; never load it on its behalf. */
    while (!(nvapi = GetModuleHandleW(L"nvapi.dll"))) {
        if (waited++ == 0)
            log_line("nvapi.dll not loaded yet, waiting");
        Sleep(500);
    }
    query = (QueryInterface_t)GetProcAddress(nvapi, "nvapi_QueryInterface");
    log_line("nvapi.dll at %p, nvapi_QueryInterface=%p", (void *)nvapi, (void *)query);
    if (!query)
        return 0;

    install("NvAPI_I2CRead", query(ID_I2C_READ), (void *)hook_i2c_read, (void **)&g_real_i2c_read);
    install("NvAPI_I2CWrite", query(ID_I2C_WRITE), (void *)hook_i2c_write, (void **)&g_real_i2c_write);
    install("NvAPI_I2CReadEx", query(ID_I2C_READ_EX), (void *)hook_i2c_read_ex, (void **)&g_real_i2c_read_ex);
    install("NvAPI_I2CWriteEx", query(ID_I2C_WRITE_EX), (void *)hook_i2c_write_ex, (void **)&g_real_i2c_write_ex);
    log_line("ready");
    return 0;
}

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved)
{
    HANDLE thread;

    (void)reserved;
    if (reason != DLL_PROCESS_ATTACH)
        return TRUE;

    /* Hooks point into this DLL, so it must never be unloaded. */
    GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                       (LPCWSTR)DllMain, &inst);
    DisableThreadLibraryCalls(inst);
    InitializeCriticalSection(&g_log_lock);
    g_tls_busy = TlsAlloc();
    log_open();
    log_line("i2c_hook loaded");
    if (MH_Initialize() != MH_OK) {
        log_line("MH_Initialize failed");
        return TRUE;
    }
    thread = CreateThread(NULL, 0, worker, NULL, 0, NULL);
    if (thread)
        CloseHandle(thread);
    return TRUE;
}
