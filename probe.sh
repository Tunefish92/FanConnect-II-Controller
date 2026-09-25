#!/usr/bin/env bash
# Phase 0 read-only hardware probe for the ASUS ROG Strix RTX 2080 Ti.
#
# By default it reads sysfs and runs nvidia-smi. It performs NO I2C transactions and
# writes nothing to the hardware.
#
#   ./probe.sh                        # run as a normal user or with sudo
#   sudo ./probe.sh --load-i2c-dev    # also runs `modprobe i2c-dev` (loads a kernel module; no hardware access)
#   sudo ./probe.sh --load-i2c-dev --read-fanconnect
#       # also reads the captured FanConnect registers (device 0x2A, see HARDWARE.md)
#       # on the target GPU's I2C buses. Uses i2cget byte-data reads only (needs i2c-tools).

set -u

want_vendor=0x10de
want_device=0x1e07
want_sub_vendor=0x1043
want_sub_device=0x866a

section() { printf '\n=== %s ===\n' "$1"; }
rd() { cat "$1" 2>/dev/null || echo "?"; }

load_i2c_dev=0
read_fanconnect=0
for arg in "$@"; do
    case "$arg" in
        --load-i2c-dev) load_i2c_dev=1 ;;
        --read-fanconnect) read_fanconnect=1 ;;
        *) echo "unknown option: $arg"; exit 2 ;;
    esac
done

if [[ $load_i2c_dev -eq 1 ]]; then
    modprobe i2c-dev || echo "modprobe i2c-dev failed (need root?)"
fi

section "System"
uname -r
if [[ -r /proc/driver/nvidia/version ]]; then
    head -n1 /proc/driver/nvidia/version
else
    echo "nvidia driver: not loaded"
fi

section "PCI"
gpu_addrs=()
for dev in /sys/bus/pci/devices/*; do
    [[ "$(rd "$dev/vendor")" == "$want_vendor" ]] || continue
    [[ "$(rd "$dev/class")" == 0x03* ]] || continue
    addr=$(basename "$dev")
    device=$(rd "$dev/device")
    sub_vendor=$(rd "$dev/subsystem_vendor")
    sub_device=$(rd "$dev/subsystem_device")
    driver=$(basename "$(readlink "$dev/driver" 2>/dev/null)" 2>/dev/null)
    match="no"
    if [[ "$device" == "$want_device" && "$sub_vendor" == "$want_sub_vendor" && "$sub_device" == "$want_sub_device" ]]; then
        match="YES (ROG Strix RTX 2080 Ti)"
        gpu_addrs+=("$addr")
    fi
    echo "$addr  ${device#0x}  subsys ${sub_vendor#0x}:${sub_device#0x}  driver=${driver:-none}  target=$match"
done
if [[ ${#gpu_addrs[@]} -eq 0 ]]; then
    echo "Target card not found."
fi

section "NVML (nvidia-smi)"
if command -v nvidia-smi >/dev/null; then
    nvidia-smi --query-gpu=name,pci.bus_id,vbios_version,temperature.gpu,utilization.gpu,power.draw,clocks.gr,clocks.mem,memory.used,memory.total,fan.speed \
        --format=csv 2>&1
else
    echo "nvidia-smi not installed"
fi

section "I2C adapters"
if lsmod 2>/dev/null | grep -q '^i2c_dev'; then
    echo "i2c-dev: loaded"
else
    echo "i2c-dev: NOT loaded (rerun with --load-i2c-dev to get /dev/i2c-* nodes)"
fi
found_i2c=0
gpu_buses=()
for bus in /sys/bus/i2c/devices/i2c-*; do
    [[ -e "$bus" ]] || continue
    found_i2c=1
    name=$(rd "$bus/name")
    parent=$(basename "$(dirname "$(readlink -f "$bus")")")
    tag=""
    for a in "${gpu_addrs[@]}"; do
        if [[ "$(readlink -f "$bus")" == *"$a"* ]]; then
            tag="  <-- on target GPU"
            gpu_buses+=("${bus##*i2c-}")
        fi
    done
    [[ "$name" == NVIDIA* && -z "$tag" ]] && tag="  <-- NVIDIA adapter"
    node="/dev/$(basename "$bus")"
    [[ -e "$node" ]] || node="(no /dev node)"
    echo "$(basename "$bus")  $node  \"$name\"  parent=$parent$tag"
done
[[ $found_i2c -eq 1 ]] || echo "No I2C adapters registered."

section "hwmon"
for h in /sys/class/hwmon/hwmon*; do
    [[ -e "$h" ]] || continue
    name=$(rd "$h/name")
    fans=$(ls "$h" 2>/dev/null | grep -E '^(fan[0-9]+_input|pwm[0-9]+)$' | tr '\n' ' ')
    echo "$(basename "$h")  $name  ${fans:-(no fan/pwm files)}"
done
echo "(The proprietary nvidia driver does not register hwmon, so no GPU entry is expected.)"

section "FanConnect (device 0x2A)"
if [[ $read_fanconnect -eq 0 ]]; then
    echo "Not read (use --read-fanconnect)."
elif ! command -v i2cget >/dev/null; then
    echo "i2cget not found (install i2c-tools)."
else
    # Registers from the GPU Tweak capture. 0x40 mode, 0x41 duty, 0x44/0x48 tach, 0x45/0x49 status.
    for n in "${gpu_buses[@]}"; do
        [[ -e /dev/i2c-$n ]] || continue
        line="i2c-$n:"
        for reg in 0x40 0x41 0x44 0x45 0x48 0x49; do
            val=$(i2cget -y "$n" 0x2a "$reg" b 2>/dev/null) || val="--"
            line+="  $reg=$val"
        done
        if [[ "$line" == *"0x45=0x01"*"0x49=0x01"* ]]; then
            line+="  <-- FanConnect candidate"
        fi
        echo "$line"
    done
    echo "(-- = no ACK / read failed. Expected on the FanConnect bus: 0x40 = 0x00 or 0x02, 0x45 = 0x49 = 0x01.)"
fi

section "Write test"
echo "NOT PERFORMED"
