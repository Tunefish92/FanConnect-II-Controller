# Capture 2026-09-25 — GPU Tweak III external fan (FanConnect II)

Tool: `tools/i2c-hook`, injected at 18:59:44 into `GPU Tweak III.exe` (pid 29908) and
`ASUSGPUFanServiceEx.exe` (pid 18148). Rem0o FanControl was closed.

User's timeline (GPU Tweak UI actions), with the matching log line:

| User note | Action                    | Log                                                      |
|-----------|---------------------------|----------------------------------------------------------|
| "19:00"   | Ext. fan duty 30%         | 19:01:04.890 GT3: 0x43=01, 0x47=01, 0x41=0x4D (the note time is off by ~1 min) |
| 19:01.19  | Ext. fan duty 60%         | 19:01:19.687 GT3: 0x41=0x99                              |
| 19:01.39  | Ext. fan duty 100%        | 19:01:38.771 GT3: 0x41=0xFF                              |
| 19:01.54  | Custom curve              | 19:01:54.529 FanServiceEx: starts 1 s loop, 0x41=0x5F/0x61 |
| 19:02.06  | Auto                      | 19:02:06.636 FanServiceEx: 0x40=00                       |
| (later)   | Auto again (no write), then reset to the curve config + Apply | 19:02:21.780 FanServiceEx: 0x40=02, then 19:02:22.288 0x43=01, 0x47=01, 0x41=0x5F; the 1 s loop resumes |

Unexplained: 19:00:07 GT3 wrote 0x43=00, 0x47=00, 0x41=00 (fans stopped: tach 0 at 19:00:11).
Probably switching to manual/fixed mode with the slider at 0% before 30% was chosen. The note's
"19:00" suggests this.

Clicking Auto a second time wrote nothing, since the mode was already 0x00.
