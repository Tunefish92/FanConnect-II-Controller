/*
 * hooktest.c - dummy target for testing inject.exe + i2c_hook.dll.
 *
 * Loads the real nvapi.dll, waits for Enter (inject during the wait), then
 * calls NvAPI_I2CReadEx with a NULL info pointer, which NVAPI rejects as an
 * invalid argument before touching any bus, and DeviceIoControl on an invalid
 * handle. Both calls should appear in the hook log.
 */

#include <windows.h>
#include <stdio.h>

typedef void *(__cdecl *QueryInterface_t)(unsigned int id);
typedef int (__cdecl *Initialize_t)(void);
typedef int (__cdecl *I2CEx_t)(void *, void *, unsigned int *);

int main(int argc, char **argv)
{
    HMODULE nvapi = LoadLibraryW(L"nvapi.dll");
    QueryInterface_t query = (QueryInterface_t)GetProcAddress(nvapi, "nvapi_QueryInterface");
    unsigned int unk = 0;
    DWORD returned = 0;

    ((Initialize_t)query(0x0150E828u))();
    if (argc > 1 && strcmp(argv[1], "--self") == 0) {
        /* Load the hook directly instead of via inject.exe; give its worker thread time to hook. */
        printf("self-load i2c_hook.dll -> %p\n", (void *)LoadLibraryW(L"i2c_hook.dll"));
        Sleep(1000);
    } else {
        printf("pid %lu ready, inject now, then press Enter\n", GetCurrentProcessId());
        fflush(stdout);
        getchar();
    }

    printf("NvAPI_I2CReadEx(NULL, NULL) -> %d\n", ((I2CEx_t)query(0x4D7B0709u))(NULL, NULL, &unk));
    printf("DeviceIoControl(invalid) -> %d\n", DeviceIoControl(INVALID_HANDLE_VALUE, 0x1234, NULL, 0, NULL, 0, &returned, NULL));
    return 0;
}
