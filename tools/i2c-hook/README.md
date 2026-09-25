# i2c-hook

Records the I2C traffic GPU Tweak III sends to the ROG Strix RTX 2080 Ti, to capture the
FanConnect II protocol.

## Why injection

Swapping in a stand-in `nvapi.dll` doesn't work. GPU Tweak III loads system DLLs only from
System32 (`SetDefaultDllDirectories`), and `Vender.dll` / `TweakInterface.dll` check signatures
(`WinVerifyTrust`). Instead, `inject.exe` loads `i2c_hook.dll` into the already-running 32-bit
GPU Tweak processes. The hook DLL hooks NVIDIA's real functions in memory using
[MinHook](https://github.com/TsudaKageyu/minhook) v1.3.4 (BSD-2, vendored in `third_party/`).
No files in the GPU Tweak folder are changed.

## What is logged

- `NvAPI_I2CWrite`, `NvAPI_I2CReadEx`, `NvAPI_I2CWriteEx`: port, device address, register bytes,
  data bytes and status. (`NvAPI_I2CRead` doesn't exist in current drivers.)
- `DeviceIoControl`: IOCTL code with the first 64 bytes in and out. This covers the ASUS `EIO`
  kernel-driver path.
- `CreateFileW/A` opens of `\\.\` device names, so IOCTL handles can be matched to devices.

The hooks only observe. Arguments and results pass through unchanged, and the DLL sends no
commands of its own.

Logs: `C:\ProgramData\gt-i2c-hook\<process>-<pid>.log`

## Build

```
tools\i2c-hook\build.cmd
```

Output: `out\i2c_hook.dll` and `out\inject.exe`, both 32-bit. They must stay in the same folder.

`hooktest.c` is a dummy target. Run `out\hooktest.exe --self` from `out\` to check the hooks without GPU Tweak.

## Capture procedure

1. Start GPU Tweak III normally. `ASUSGPUFanServiceEx.exe` (the external fan service) should be
   running too.
2. In an **elevated** PowerShell, from the repo folder:
   ```powershell
   & .\tools\i2c-hook\out\inject.exe
   ```
   It injects into `GPU Tweak III.exe`, `ASUSGPUFanServiceEx.exe` and `ASUSGPUFanService.exe`,
   whichever are running. You can also pass process names or PIDs.
3. Check that each log in `C:\ProgramData\gt-i2c-hook\` ends with `ready`.
4. Wait 30 seconds, then change one external fan setting at a time. Wait about 10 seconds after
   each change and write down the clock time.
5. Done. No cleanup is needed: the hooks disappear when GPU Tweak and its services restart, or
   at reboot. Until then they only observe.

If a GPU Tweak process was started after step 2, run `inject.exe` again. The log then has a new PID.
