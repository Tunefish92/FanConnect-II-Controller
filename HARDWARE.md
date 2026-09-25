# Hardware notes — ASUS ROG Strix RTX 2080 Ti FanConnect II

Phase 0 findings. Everything here was gathered read-only.

## Card identity (Windows, `nvidia-smi`, 2026-09-25)

| Field            | Value                          |
|------------------|--------------------------------|
| GPU              | NVIDIA GeForce RTX 2080 Ti     |
| PCI ID           | `10de:1e07`                    |
| Subsystem        | `1043:866a` (ASUS)             |
| PCI bus (Win)    | `0A:00.0`                      |
| VBIOS            | 90.02.17.00.47 (`...AS18`)     |
| GPU Tweak model  | `ROG-STRIX-RTX2080TI`          |

`1043:866a` is the value detection should match on: vendor `10de`, device `1e07`, subsystem vendor `1043`, subsystem device `866a`.

## How GPU Tweak III reaches the board

Installed at `C:\Program Files (x86)\ASUS\GPUTweakIII`. What the binaries show:

- `Vender.dll` exports `VGA_Read_I2C` / `VGA_Write_I2C` / `ReadI2C` / `WriteI2C` and resolves
  NVIDIA functions with `nvapi_QueryInterface`. This is the NVAPI I2C path
  (`NvAPI_I2CReadEx` / `NvAPI_I2CWriteEx`), which talks over the GPU's own I2C buses.
- `Vender.dll` loads NVAPI by bare name (`"nvapi.dll"`). It is **32-bit**, and so are
  `GPU Tweak III.exe` and `ASUSGPUFanService*.exe`. So the 32-bit `SysWOW64\nvapi.dll` is used.
  It exports only `nvapi_QueryInterface` and `nvapi_Direct_GetMethod`.
- External fan control goes through `Get/Set_LED_ExternalFan_Info` in `VGA_Extra.dll` and
  `TweakInterface.dll`. It sits in the same module as the RGB/LED code, which uses `Vender.dll`'s
  `ReadI2C` / `WriteI2C`.
- `ITECCTdll.dll` (ITE Tech) imports `HID.DLL`, so it drives a USB HID device. It is probably
  **not** related to the graphics card.
- **`ASUSGPUFanServiceEx.exe` is the external fan service.** Its strings include
  `ExternalFanService`, `ExternalFanDuty1 Mode`, `ExternalFanMinDuty` and
  `Tune: ExternalFan Service Behavior`. It calls `VGA_Extra.dll!Set_LED_ExternalFan_Info`,
  which uses `Vender.dll`.
- `ASUSGPUFanService.exe` (without "Ex") uses `Exeio.dll!SetFanDuty_ByReg` /
  `ReSetFanDuty_ByReg`. That is GPU register access through the `EIO` kernel driver, most
  likely for the GPU's own fans. `EIO.dll` also exports MMIO-based `ReadI2C` / `WriteI2C`,
  so the capture tool logs IOCTLs too.
- `InitData_Card1.ini` lists two fan tables (`[fansetting]`, `[fansetting2]`) with
  `IsSupportFanPolicyTable=1`, so fan-curve support is built in for this card.
- `EIO64.sys` / `IOMap64.sys` (port and memory I/O drivers) ship too, but nothing points to them
  being used for fans on NVIDIA cards.

**Conclusion:** FanConnect II is almost certainly an I2C device on one of the GPU's I2C buses.
It is probably not a PCI BAR, an ACPI device or an x86 I/O-port controller.

## What this means for Linux

- The proprietary `nvidia` driver registers the GPU's I2C buses as Linux I2C adapters, named
  `NVIDIA i2c adapter N at 0:xx.0`. With `i2c-dev` loaded they appear as `/dev/i2c-*`.
  This is how OpenRGB reaches ASUS GPU RGB controllers on Linux.
- The proprietary driver does **not** register a hwmon device for the GPU. Expect the hwmon
  check to show nothing for FanConnect.
- The ASUS laptop EC ports (`0x25C/0x25D`) and `AsIO2`-style port I/O do not apply. Don't touch them.
- The RGB controller probably shares a bus with the fan MCU. A write to the wrong address or
  register could change RGB state or worse, so writes need an exact, captured protocol.

## Plan changes

1. Skip reverse-engineering Windows drivers. **Capture the I2C transactions on Windows instead**,
   on this machine, with [tools/i2c-hook](tools/i2c-hook/README.md). It injects logging hooks
   into the running GPU Tweak processes. While it logs, change the external fan duty in
   GPU Tweak (for example 30% → 60% → 100% → auto) and note the times. The captured writes are
   the protocol.
   (A stand-in `nvapi.dll` in the GPU Tweak folder was tried first and was never loaded.
   GPU Tweak uses `SetDefaultDllDirectories` and `WinVerifyTrust`.)
2. Then on CachyOS, `probe.sh` finds the matching I2C adapter. After that, `i2c-dev` reads of
   **only the captured registers** confirm the device before any write code exists.
3. **Never** run `i2cdetect` with default probing, or `sensors-detect`, on the GPU buses.
   Blind probing can upset MCUs and is the unsafe experimentation the plan warns against.

## Captured protocol (2026-09-25)

Source: [captures/2026-09-25-gputweak](captures/2026-09-25-gputweak/NOTES.md). All traffic uses
`NvAPI_I2CReadEx` / `NvAPI_I2CWriteEx`. Every transaction is a 1-byte register address followed by 1 data byte.

**Device:** NVAPI I2C **port 1**, 7-bit address **0x2A** (8-bit `0x54`), speed setting 4
(`NVAPI_I2C_SPEED_100KHZ`?), `bIsDDCPort=0`. No other I2C device was touched.

| Reg  | Access | Meaning (inferred)                          | Evidence |
|------|--------|---------------------------------------------|----------|
| 0x40 | R/W    | Control mode: `0x02` = host/software, `0x00` = auto | The service reads it before every write; clicking Auto wrote `0x00` |
| 0x41 | R/W    | **External fan duty, 0–255, shared by both fans** | 30% → `0x4D`, 60% → `0x99`, 100% → `0xFF` |
| 0x43 | W      | Fan 1 output enable (`01` on / `00` off)    | Written together with 0x47 |
| 0x44 | R      | Fan 1 tach, **RPM = value × 30**            | `0x19` (750 RPM) at 30%, `0x2F` (1410) at 60%, `0x48` (2160) at 100%, `0` when stopped |
| 0x45 | R      | Fan 1 status, reads `01` (present?)         | Read before 0x43 is written |
| 0x47 | W      | Fan 2 output enable                         | Same pattern as 0x43 |
| 0x48 | R      | Fan 2 tach, RPM = value × 30                | Tracks 0x44 |
| 0x49 | R      | Fan 2 status, reads `01`                    | Read before 0x47 is written |

Observed sequences:

- **Set fixed duty** (GPU Tweak III.exe): read 0x45, write 0x43=01, read 0x49, write 0x47=01,
  write 0x41=duty.
- **Custom curve** (ASUSGPUFanServiceEx.exe): about once a second, read 0x40 (expect `02`),
  read 0x41, write 0x41=curve duty. **The curve runs on the PC, not on the card.**
- **Auto:** write 0x40=`00`. Afterwards 0x41 read `00` and both tachs read 0 (fans stopped at idle).
- **Back to curve (Apply):** write 0x40=`02`, then the fixed-duty sequence (0x43=01, 0x47=01,
  0x41=duty), then the 1 s loop again. So entering host control is: 0x40=02, enable both outputs, set duty.

Tach reads happen about every 3 s from GPU Tweak III.exe.

## Open questions

- [x] I2C port/bus index and 7-bit address: NVAPI port 1, 0x2A
- [x] Duty register: 0x41, one register for both fans (0x42/0x46 unexplored; per-fan duty may not exist)
- [x] Tach unit: RPM = value × 30 (GPU Tweak showed 2160 RPM at 100% while the register read 0x48 = 72)
- [x] ~~Auto mode (0x40=00) behavior~~: not needed. The daemon never uses auto mode (see [DESIGN.md](DESIGN.md))
- [x] 19:02:21 return to host mode: the user re-applied the curve config
- [ ] 19:00:07 "all off" write (0x43=0x47=0x41=00). Probably entering fixed mode with the slider at 0%
- [x] Linux bus: the adapter named **`NVIDIA i2c adapter 1 at a:00.0`** (NVAPI port N ↔ "adapter N"). It was `/dev/i2c-4` on 2026-09-25 (CachyOS, kernel 7.1.6, NVIDIA open module 610.57.04). Bus numbers can change, so match by name and parent PCI device
- [x] ~~"Restore default/auto"~~: not needed (`ReSetFanDuty_ByReg` belongs to the GPU's own fans anyway)
- [ ] Does the MCU follow GPU temperature by itself when released? (That decides what a
      daemon crash leaves behind.)
- [ ] Which process writes: `GPU Tweak III.exe` or `ASUSGPUFanService*.exe`?
- [ ] With the CPU or Mixed temperature source, does GPU Tweak push temperatures to the card
      periodically? That would mean the card runs the curve itself.

## Linux verification (2026-09-25, read-only)

`probe.sh --read-fanconnect` on CachyOS. Only `i2c-4` ("NVIDIA i2c adapter 1 at a:00.0") answers at 0x2A:

```
i2c-4:  0x40=0x02  0x41=0x64  0x44=0x20  0x45=0x01  0x48=0x20  0x49=0x01
```

With no controller running, the card sat in host mode (`02`) at duty `0x64` (39 %), both fans at
960 RPM. It isn't known yet whether that is a power-on default or state kept from the Windows
session.

## Linux write test (2026-09-25) — works

`tools/write-test.sh` on CachyOS (live), bus `i2c-4`. Sequence: 0x40=02, 0x43=01, 0x47=01, then 0x41:

```
Before: mode=0x02 duty=0x66 (40 %), fan1 990 RPM, fan2 990 RPM
Set  30 % (0x4D) -> readback 0x4d, fan1  750 RPM, fan2  750 RPM
Set  60 % (0x99) -> readback 0x99, fan1 1410 RPM, fan2 1410 RPM
Set 100 % (0xFF) -> readback 0xff, fan1 2160 RPM, fan2 2130 RPM
Restored duty 0x66
```

The RPMs match the Windows capture exactly, so both FanConnect II headers can be controlled from Linux.

**The firmware does not change the duty by itself.** With nothing writing, 0x41 stayed constant while
the GPU heated up under load and cooled back to about 40 °C (checked 2026-09-25 with `watch i2cget`).
The earlier change from `0x64` to `0x66` was most likely GPU Tweak's curve service during a Windows boot in between.
**The chip keeps the last written duty and mode across reboots.** So whatever wrote last (GPU Tweak or this
daemon) sets the fan speed until something writes again.

## Windows access from gpu-fanctl (2026-09-25)

`gpu-fanctl.exe` (64-bit) loads `nvapi64.dll`, finds the card through `NvAPI_GPU_GetPCIIdentifiers`
(`0x1E0710DE` / `0x866A1043`), and calls `NvAPI_I2CReadEx` / `NvAPI_I2CWriteEx` with the captured
parameters: port 1, address `0x54`, 1-byte register, 1 data byte, `i2cSpeedKhz` 4. Reads and writes
work **without administrator rights**. The first run read mode `0x02`, duty `0x64`, 960/960 RPM
(the same values as Linux) and wrote the same values back successfully.
