//! The FanConnect II controller on supported cards (the ROG Strix RTX 2080 Ti): I2C device 0x2A
//! on the GPU's I2C port 1 ("NVIDIA i2c adapter 1" on Linux, NVAPI port 1 on Windows).
//! Register map in HARDWARE.md.

use std::io;

use anyhow::{Context, Result, bail};

use crate::curve;

/// 7-bit I2C address of the controller.
pub const ADDRESS: u8 = 0x2a;
/// The GPU I2C port the controller sits on.
pub const PORT: u8 = 1;

/// A graphics card whose FanConnect II protocol has been verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupportedCard {
    /// PCI device ID as `device << 16 | vendor`.
    pub device_id: u32,
    /// PCI subsystem ID as `subsystem device << 16 | subsystem vendor`.
    pub subsystem_id: u32,
    pub name: &'static str,
}

/// Cards this software controls. Only add a card after its protocol has been verified
/// (see HARDWARE.md): a wrong write can reach another chip on the same I2C bus.
pub const SUPPORTED_CARDS: &[SupportedCard] =
    &[SupportedCard { device_id: 0x1E07_10DE, subsystem_id: 0x866A_1043, name: "ASUS ROG Strix GeForce RTX 2080 Ti" }];

/// The supported card with these PCI IDs, if any.
pub fn supported_card(device_id: u32, subsystem_id: u32) -> Option<&'static SupportedCard> {
    SUPPORTED_CARDS.iter().find(|c| c.device_id == device_id && c.subsystem_id == subsystem_id)
}

/// PCI IDs in the usual `vendor:device, subsystem vendor:device` form, e.g. `10de:1e07, 1043:866a`.
pub fn format_pci_ids(device_id: u32, subsystem_id: u32) -> String {
    format!(
        "{:04x}:{:04x}, subsystem {:04x}:{:04x}",
        device_id & 0xFFFF,
        device_id >> 16,
        subsystem_id & 0xFFFF,
        subsystem_id >> 16
    )
}

const REG_MODE: u8 = 0x40;
const REG_DUTY: u8 = 0x41;
const REG_FAN1_ENABLE: u8 = 0x43;
const REG_FAN1_TACH: u8 = 0x44;
const REG_FAN1_STATUS: u8 = 0x45;
const REG_FAN2_ENABLE: u8 = 0x47;
const REG_FAN2_TACH: u8 = 0x48;
const REG_FAN2_STATUS: u8 = 0x49;

pub const MODE_AUTO: u8 = 0x00;
pub const MODE_HOST: u8 = 0x02;
const RPM_PER_TACH_UNIT: u32 = 30;

/// Register access to the controller, one byte per transfer.
pub trait Transport {
    fn read(&self, reg: u8) -> io::Result<u8>;
    fn write(&self, reg: u8, value: u8) -> io::Result<()>;
    /// Where the controller is, for messages (e.g. "i2c-4, GPU 0000:0a:00.0").
    fn describe(&self) -> String;
    /// PCI bus number of the graphics card the controller belongs to.
    fn pci_bus(&self) -> u32;
    /// The supported card the controller was found on.
    fn card(&self) -> &'static SupportedCard;
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub mode: u8,
    pub duty_reg: u8,
    pub fan1_rpm: u32,
    pub fan2_rpm: u32,
}

impl Status {
    pub fn duty_percent(&self) -> f32 {
        curve::reg_to_duty(self.duty_reg)
    }
}

pub struct FanConnect {
    transport: Box<dyn Transport>,
}

impl FanConnect {
    /// Finds, opens and identifies the controller. Nothing is written.
    pub fn open() -> Result<Self> {
        #[cfg(target_os = "linux")]
        let transport: Box<dyn Transport> = Box::new(crate::i2c::open()?);
        #[cfg(windows)]
        let transport: Box<dyn Transport> = Box::new(crate::nvapi::open()?);
        let fc = Self { transport };
        fc.identify()?;
        Ok(fc)
    }

    pub fn describe(&self) -> String {
        self.transport.describe()
    }

    /// PCI bus of the card, so the GPU temperature is read from the same card.
    pub fn pci_bus(&self) -> u32 {
        self.transport.pci_bus()
    }

    pub fn card(&self) -> &'static SupportedCard {
        self.transport.card()
    }

    fn read(&self, reg: u8) -> Result<u8> {
        self.transport
            .read(reg)
            .with_context(|| format!("reading register 0x{reg:02X} ({})", self.describe()))
    }

    fn write(&self, reg: u8, value: u8) -> Result<()> {
        self.transport
            .write(reg, value)
            .with_context(|| format!("writing register 0x{reg:02X}=0x{value:02X} ({})", self.describe()))
    }

    /// Checks the registers that identify the controller before anything is written.
    fn identify(&self) -> Result<()> {
        let (s1, s2, mode) = (self.read(REG_FAN1_STATUS)?, self.read(REG_FAN2_STATUS)?, self.read(REG_MODE)?);
        if s1 != 0x01 || s2 != 0x01 || !(mode == MODE_AUTO || mode == MODE_HOST) {
            bail!(
                "device 0x{ADDRESS:02X} ({}) does not look like FanConnect II \
                 (status 0x{s1:02X}/0x{s2:02X}, mode 0x{mode:02X}; expected 0x01/0x01 and mode 0x00 or 0x02)",
                self.describe()
            );
        }
        Ok(())
    }

    pub fn status(&self) -> Result<Status> {
        Ok(Status {
            mode: self.read(REG_MODE)?,
            duty_reg: self.read(REG_DUTY)?,
            fan1_rpm: u32::from(self.read(REG_FAN1_TACH)?) * RPM_PER_TACH_UNIT,
            fan2_rpm: u32::from(self.read(REG_FAN2_TACH)?) * RPM_PER_TACH_UNIT,
        })
    }

    pub fn mode(&self) -> Result<u8> {
        self.read(REG_MODE)
    }

    pub fn duty_reg(&self) -> Result<u8> {
        self.read(REG_DUTY)
    }

    /// Host control with both outputs enabled, the sequence GPU Tweak III uses.
    pub fn take_control(&self) -> Result<()> {
        self.write(REG_MODE, MODE_HOST)?;
        self.write(REG_FAN1_ENABLE, 0x01)?;
        self.write(REG_FAN2_ENABLE, 0x01)
    }

    pub fn set_duty(&self, percent: f32) -> Result<()> {
        self.write(REG_DUTY, curve::duty_to_reg(percent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_cards_are_looked_up_by_both_ids() {
        assert_eq!(supported_card(0x1E07_10DE, 0x866A_1043).map(|c| c.name), Some("ASUS ROG Strix GeForce RTX 2080 Ti"));
        // Same GPU from another board maker, or another ASUS model: not supported.
        assert_eq!(supported_card(0x1E07_10DE, 0x1234_1462), None);
        assert_eq!(supported_card(0x1E04_10DE, 0x866A_1043), None);
    }

    #[test]
    fn pci_ids_are_formatted_vendor_first() {
        assert_eq!(format_pci_ids(0x1E07_10DE, 0x866A_1043), "10de:1e07, subsystem 1043:866a");
    }
}
