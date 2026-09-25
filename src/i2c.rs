//! Linux transport: i2c-dev SMBus "byte data" transfers, the same ones
//! `i2cget -y BUS ADDR REG b` / `i2cset -y BUS ADDR REG VALUE b` perform.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::fanconnect::{ADDRESS, PORT, SupportedCard, Transport, format_pci_ids, supported_card};

// From <linux/i2c-dev.h> and <linux/i2c.h>.
const I2C_SLAVE: libc::c_ulong = 0x0703;
const I2C_SMBUS: libc::c_ulong = 0x0720;
const I2C_SMBUS_READ: u8 = 1;
const I2C_SMBUS_WRITE: u8 = 0;
const I2C_SMBUS_BYTE_DATA: u32 = 2;
const I2C_SMBUS_BLOCK_MAX: usize = 32;

#[repr(C)]
union SmbusData {
    byte: u8,
    word: u16,
    block: [u8; I2C_SMBUS_BLOCK_MAX + 2],
}

#[repr(C)]
struct SmbusIoctlData {
    read_write: u8,
    command: u8,
    size: u32,
    data: *mut SmbusData,
}

pub struct I2cDevice {
    file: File,
    bus: u32,
    pci: String,
    card: &'static SupportedCard,
}

fn read_trimmed(path: &Path) -> String {
    fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn sysfs_id(path: &Path) -> Option<u32> {
    u32::from_str_radix(read_trimmed(path).trim_start_matches("0x"), 16).ok()
}

/// Finds the bus by adapter name ("NVIDIA i2c adapter <PORT> at ...") on a supported card.
/// Bus numbers can change between boots, so they are never hardcoded.
fn locate() -> Result<(u32, String, &'static SupportedCard)> {
    let prefix = format!("NVIDIA i2c adapter {PORT} at ");
    let entries = fs::read_dir("/sys/bus/i2c/devices").context("reading /sys/bus/i2c/devices")?;
    let mut unsupported = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(bus) = name.strip_prefix("i2c-").and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if !read_trimmed(&entry.path().join("name")).starts_with(&prefix) {
            continue;
        }
        let Some(pci) = fs::canonicalize(entry.path())
            .ok()
            .and_then(|p| p.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()))
        else {
            continue;
        };
        let dev = Path::new("/sys/bus/pci/devices").join(&pci);
        let id = |f: &str| sysfs_id(&dev.join(f));
        let device_id = id("device").zip(id("vendor")).map(|(d, v)| d << 16 | v).unwrap_or(0);
        let subsystem_id = id("subsystem_device").zip(id("subsystem_vendor")).map(|(d, v)| d << 16 | v).unwrap_or(0);
        match supported_card(device_id, subsystem_id) {
            Some(card) => return Ok((bus, pci, card)),
            None => unsupported.push(format!("{pci} ({})", format_pci_ids(device_id, subsystem_id))),
        }
    }
    if unsupported.is_empty() {
        bail!(
            "no \"{}\" found. Is the nvidia driver loaded and i2c-dev available (modprobe i2c-dev)?",
            prefix.trim_end()
        )
    }
    bail!("no supported graphics card found; not supported: {}", unsupported.join("; "))
}

/// PCI bus number from a PCI address like `0000:0a:00.0`.
fn bus_number(pci: &str) -> u32 {
    pci.split(':').nth(1).and_then(|b| u32::from_str_radix(b, 16).ok()).unwrap_or(0)
}

/// Locates the controller's bus and opens it with the controller's address selected.
pub fn open() -> Result<I2cDevice> {
    let (bus, pci, card) = locate()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("/dev/i2c-{bus}"))
        .with_context(|| format!("opening /dev/i2c-{bus} (run as root)"))?;
    // SAFETY: I2C_SLAVE takes the address as an integer argument.
    if unsafe { libc::ioctl(file.as_raw_fd(), I2C_SLAVE as _, libc::c_ulong::from(ADDRESS)) } < 0 {
        return Err(io::Error::last_os_error()).context(format!("selecting address 0x{ADDRESS:02X} on i2c-{bus}"));
    }
    Ok(I2cDevice { file, bus, pci, card })
}

impl I2cDevice {
    fn smbus(&self, read_write: u8, command: u8, data: &mut SmbusData) -> io::Result<()> {
        let mut args = SmbusIoctlData { read_write, command, size: I2C_SMBUS_BYTE_DATA, data };
        // SAFETY: args and the data it points to outlive the call.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_SMBUS as _, &mut args) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Transport for I2cDevice {
    fn read(&self, reg: u8) -> io::Result<u8> {
        let mut data = SmbusData { block: [0; I2C_SMBUS_BLOCK_MAX + 2] };
        self.smbus(I2C_SMBUS_READ, reg, &mut data)?;
        // SAFETY: every field of the union is plain bytes; the kernel filled in `byte`.
        Ok(unsafe { data.byte })
    }

    fn write(&self, reg: u8, value: u8) -> io::Result<()> {
        let mut data = SmbusData { block: [0; I2C_SMBUS_BLOCK_MAX + 2] };
        data.byte = value;
        self.smbus(I2C_SMBUS_WRITE, reg, &mut data)
    }

    fn describe(&self) -> String {
        format!("i2c-{}, GPU {}", self.bus, self.pci)
    }

    fn pci_bus(&self) -> u32 {
        bus_number(&self.pci)
    }

    fn card(&self) -> &'static SupportedCard {
        self.card
    }
}
