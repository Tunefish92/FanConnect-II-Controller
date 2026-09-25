/*
 * inject.c - loads i2c_hook.dll (from this exe's folder) into running 32-bit
 * GPU Tweak III processes. Must run elevated.
 *
 *   inject.exe                  inject into the default GPU Tweak processes
 *   inject.exe name.exe|pid ... inject into the named processes or PIDs instead
 *
 * The hook DLL pins itself, so it stays until the process exits.
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>

static const wchar_t *default_targets[] = {
    L"GPU Tweak III.exe",
    L"ASUSGPUFanServiceEx.exe",
    L"ASUSGPUFanService.exe",
};

static int inject(DWORD pid, const wchar_t *dll_path)
{
    SIZE_T bytes = (wcslen(dll_path) + 1) * sizeof(wchar_t);
    HANDLE proc, thread;
    BOOL wow64 = FALSE;
    void *remote;
    DWORD result = 0;

    proc = OpenProcess(PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION |
                       PROCESS_VM_WRITE | PROCESS_VM_READ, FALSE, pid);
    if (!proc) {
        wprintf(L"  OpenProcess failed: %lu (run elevated?)\n", GetLastError());
        return 0;
    }
    if (!IsWow64Process(proc, &wow64) || !wow64) {
        wprintf(L"  not a 32-bit process, skipped\n");
        CloseHandle(proc);
        return 0;
    }
    remote = VirtualAllocEx(proc, NULL, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!remote || !WriteProcessMemory(proc, remote, dll_path, bytes, NULL)) {
        wprintf(L"  writing DLL path failed: %lu\n", GetLastError());
        CloseHandle(proc);
        return 0;
    }
    /* kernel32 is mapped at the same address in every 32-bit process of this session. */
    thread = CreateRemoteThread(proc, NULL, 0,
                                (LPTHREAD_START_ROUTINE)GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "LoadLibraryW"),
                                remote, 0, NULL);
    if (!thread) {
        wprintf(L"  CreateRemoteThread failed: %lu\n", GetLastError());
    } else {
        WaitForSingleObject(thread, 10000);
        GetExitCodeThread(thread, &result);
        CloseHandle(thread);
        wprintf(result ? L"  injected\n" : L"  LoadLibraryW failed in target\n");
    }
    VirtualFreeEx(proc, remote, 0, MEM_RELEASE);
    CloseHandle(proc);
    return result != 0;
}

int wmain(int argc, wchar_t **argv)
{
    const wchar_t **targets = argc > 1 ? (const wchar_t **)argv + 1 : default_targets;
    int target_count = argc > 1 ? argc - 1 : (int)(sizeof default_targets / sizeof *default_targets);
    wchar_t dll_path[MAX_PATH], *slash;
    PROCESSENTRY32W pe = { sizeof pe };
    HANDLE snap;
    int i, found = 0, ok = 0;

    GetModuleFileNameW(NULL, dll_path, MAX_PATH);
    slash = wcsrchr(dll_path, L'\\');
    wcscpy_s(slash + 1, MAX_PATH - (slash + 1 - dll_path), L"i2c_hook.dll");
    if (GetFileAttributesW(dll_path) == INVALID_FILE_ATTRIBUTES) {
        wprintf(L"%s not found\n", dll_path);
        return 1;
    }

    for (i = 0; i < target_count; i++) {
        wchar_t *end;
        DWORD pid = wcstoul(targets[i], &end, 10);
        if (pid && !*end) {
            found++;
            wprintf(L"pid %lu\n", pid);
            ok += inject(pid, dll_path);
        }
    }

    snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    for (BOOL more = Process32FirstW(snap, &pe); more; more = Process32NextW(snap, &pe)) {
        for (i = 0; i < target_count; i++) {
            if (_wcsicmp(pe.szExeFile, targets[i]) == 0) {
                found++;
                wprintf(L"%s (pid %lu)\n", pe.szExeFile, pe.th32ProcessID);
                ok += inject(pe.th32ProcessID, dll_path);
            }
        }
    }
    CloseHandle(snap);
    wprintf(L"%d of %d processes injected. Logs: C:\\ProgramData\\gt-i2c-hook\\\n", ok, found);
    return ok ? 0 : 1;
}
