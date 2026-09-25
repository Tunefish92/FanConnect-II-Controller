#!/usr/bin/env bash
# First FanConnect II write test (Linux). Replays exactly what GPU Tweak III was seen doing:
#   0x40=0x02 (host control), 0x43=0x01 / 0x47=0x01 (outputs on), 0x41=duty.
# Steps 30 % -> 60 % -> 100 %, holds each for a few seconds, reads back duty and RPM,
# then restores the original 0x40/0x41 values (also on Ctrl+C or error).
#
#   sudo ./tools/write-test.sh

set -u

addr=0x2a
hold=8

die() { echo "ABORT: $*"; exit 1; }

command -v i2cset >/dev/null || die "i2c-tools not installed"
[[ $EUID -eq 0 ]] || die "run with sudo"
modprobe i2c-dev 2>/dev/null

# Find the bus by name on the target card, not by number.
bus=""
for b in /sys/bus/i2c/devices/i2c-*; do
    [[ "$(cat "$b/name" 2>/dev/null)" == "NVIDIA i2c adapter 1 at "* ]] || continue
    pci=$(basename "$(dirname "$(readlink -f "$b")")")
    [[ "$(cat /sys/bus/pci/devices/$pci/subsystem_vendor 2>/dev/null):$(cat /sys/bus/pci/devices/$pci/subsystem_device 2>/dev/null)" == "0x1043:0x866a" ]] || continue
    bus=${b##*i2c-}
done
[[ -n "$bus" ]] || die "NVIDIA i2c adapter 1 on the ROG Strix 2080 Ti not found"

rd() { i2cget -y "$bus" $addr "$1" b 2>/dev/null; }
wr() { i2cset -y "$bus" $addr "$1" "$2" b || die "write $1=$2 failed"; }
rpm() { local v; v=$(rd "$1") && echo $(( v * 30 )); }

# Identify the device before writing anything.
mode=$(rd 0x40) || die "no answer from $addr on i2c-$bus"
duty=$(rd 0x41) || die "cannot read duty"
[[ "$(rd 0x45)" == 0x01 && "$(rd 0x49)" == 0x01 ]] || die "status registers 0x45/0x49 are not 0x01; not the expected device"
[[ "$mode" == 0x00 || "$mode" == 0x02 ]] || die "unexpected mode $mode"

echo "Bus i2c-$bus. Before: mode=$mode duty=$duty ($(( duty * 100 / 255 )) %), fan1 $(rpm 0x44) RPM, fan2 $(rpm 0x48) RPM"

restore() {
    trap - EXIT INT TERM
    echo "Restoring mode=$mode duty=$duty"
    i2cset -y "$bus" $addr 0x41 "$duty" b
    [[ "$mode" == 0x02 ]] || i2cset -y "$bus" $addr 0x40 "$mode" b
    echo "After:  mode=$(rd 0x40) duty=$(rd 0x41), fan1 $(rpm 0x44) RPM, fan2 $(rpm 0x48) RPM"
}
trap restore EXIT INT TERM

wr 0x40 0x02
wr 0x43 0x01
wr 0x47 0x01

for pct in 30 60 100; do
    val=$(( (pct * 255 + 50) / 100 ))
    wr 0x41 "$val"
    printf "Set %3d %% (0x%02X), waiting %ds..." "$pct" "$val" "$hold"
    sleep "$hold"
    echo " readback duty=$(rd 0x41), fan1 $(rpm 0x44) RPM, fan2 $(rpm 0x48) RPM"
done
