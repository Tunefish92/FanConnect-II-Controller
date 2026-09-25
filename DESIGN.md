# Design decisions

Register details are in [HARDWARE.md](HARDWARE.md).

## Control mode: always our own curve

The daemon always drives the external fans from a curve: the built-in Auto curve by default, or
a custom curve the user sets. The card's own auto mode is not used.

- On start: write 0x40=`02` (host control), 0x43=`01` and 0x47=`01` (both outputs on), then the
  curve duty to 0x41.
- The daemon **never writes 0x40=`00`**. The card's auto mode is not used.
- In every cycle, read 0x40. If it isn't `02` (another tool or a card reset changed it), redo the
  start sequence. GPU Tweak's service does the same check.

## One curve for both fans

The hardware has a single duty register (0x41) for both FanConnect outputs, so there is one curve.
Independent per-fan curves are not possible.

## Settings

Stored in `/etc/gpu-fan-controller.conf` (Linux) or `%ProgramData%\gpu-fanctl\gpu-fan-controller.conf`
(Windows). A running daemon reloads the file within a second of a change.

```
max_temp = 80
curve = 40:30, 55:40, 65:60, 75:85, 80:100     # optional; without it the Auto curve is used
```

### Max GPU temp

- **Default 80 °C. Allowed range 60–90 °C.** Set with `gpu-fanctl max-temp <°C>`.
- **At or above max temp the fans always run at 100 %**, whichever curve is active. This is the
  safety limit.
- For the Auto curve, max temp is also where the curve ends: the curve stretches between 45 °C
  and max temp.

### Custom curve

- `temperature:duty` points (°C:%), set with `gpu-fanctl curve set 40:30 55:40 65:60 75:85 80:100`
  or by editing the file. `gpu-fanctl curve auto` goes back to the Auto curve.
- Rules: 2–16 points, temperatures 20–100 °C and strictly rising, duty 30–100 % and never
  falling. Linear interpolation between points. Below the first point the first duty applies;
  above the last point, the last one.
- The smoothing below and the max temp override apply to custom curves too.
- **Invalid settings file:** the daemon uses the Auto curve with max temp 80 and logs a warning.
  The `curve`/`max-temp` commands refuse to overwrite an invalid file, so hand edits aren't lost.

### Auto curve (built-in)

"Auto" in this project means our own built-in curve, not the card's auto mode.

The curve starts at a fixed 45 °C and ends at max temp. Each point sits at a fixed fraction
of that span:

`temp = 45 + fraction × (max_temp − 45)`

Duty is interpolated linearly between points. At or below 45 °C it is 30 %; at or above max temp,
100 %.

| Fraction | Duty  | Reg 0x41 | ≈ RPM | Temp at max 80 (default)  | Temp at max 85 |
|----------|-------|----------|-------|---------------------------|----------------|
| 0        | 30 %  | `0x4D`   | 750   | ≤ 45 °C (idle/desktop)    | ≤ 45 °C        |
| 0.25     | 35 %  | `0x59`   | 850   | 54 °C (light load)        | 55 °C          |
| 0.40     | 45 %  | `0x73`   | 1050  | 59 °C                     | 61 °C          |
| 0.55     | 55 %  | `0x8C`   | 1250  | 64 °C (gaming starts)     | 67 °C          |
| 0.70     | 70 %  | `0xB3`   | 1550  | 70 °C (typical gaming)    | 73 °C          |
| 0.85     | 85 %  | `0xD9`   | 1850  | 75 °C (heavy gaming)      | 79 °C          |
| 1        | 100 % | `0xFF`   | 2160  | ≥ 80 °C                   | ≥ 85 °C        |

Duty % maps to the register as `round(percent × 255 / 100)`. This matches GPU Tweak: 30 % → 0x4D.
RPM values are estimates from the 30/60/100 % capture.

The default of 80 °C puts full airflow a few degrees below the point where the 2080 Ti starts
reducing clocks (about 84 °C). Setting max temp above 84 °C means the external fans reach 100 %
only once the card is already throttling.

**Floor: 30 %.** The fans never go below 30 % and are never stopped. 30 % is the lowest duty
seen running reliably (750 RPM); lower duties weren't tested.

### Behaviour

Rise fast, fall slowly, so the fans don't pump up and down with short load changes (loading
screens, alt-tab):

1. **Poll:** read the GPU core temperature through NVML once per second.
2. **Rising:** when the curve gives a higher duty than the current one, apply it right away.
3. **Falling with hysteresis:** the curve reads an *effective* temperature. It follows rising
   temperatures immediately. It ignores drops of up to **3 °C**, and after a larger drop it
   stays 3 °C above the real temperature. Wobbling by a degree or two changes nothing.
4. **Ramp down:** when falling, lower the duty by at most **2 % per second**. From 100 % to
   30 % that takes about 35 s.
5. **Hot:** at or above max temp, go to 100 % right away, skipping the ramp and hysteresis.
6. **Write every cycle:** write 0x41 once per second even if the value hasn't changed. The firmware
   doesn't drift, but this matches GPU Tweak and corrects any other writer within a second.

## Fail-safe (replaces "restore auto")

Without auto mode, the fans hold whatever duty was last written. So every failure goes to a fixed
**fail-safe duty (default 100 %)**:

| Situation | Action |
|-----------|--------|
| GPU temperature can't be read (NVML error) | Write the fail-safe duty; retry NVML every 5 s |
| Daemon stops (Linux: SIGTERM/SIGINT/SIGHUP, `systemctl stop`, shutdown; Windows: service stop or system shutdown; console: Ctrl+C) | Write the fail-safe duty, then exit |
| Daemon crashes | Can't write anything, so fans keep the last duty. It comes back within seconds: systemd `Restart=always` (Linux), the service's restart-on-failure actions (Windows) |
| Controller or driver not reachable (e.g. early at boot) | Linux: exit, systemd restarts it every 3 s. Windows: the service retries every 5 s |
| I2C write fails | Log it, retry next cycle; after 10 failures in a row, write the fail-safe duty and exit (then restarted as above) |

Before the daemon starts at boot, the card keeps the last mode and duty written, even across
reboots. That may come from GPU Tweak on Windows or from this daemon's own fail-safe write.
