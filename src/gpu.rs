//! GPU data through NVIDIA's NVML, loaded at runtime (libnvidia-ml.so.1 on Linux, nvml.dll on
//! Windows): the temperature for the control loop, and a list of all NVIDIA cards with their
//! identity for the GUI and `gpu-fanctl detect`.

use anyhow::{Context, Result, bail};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

use crate::fanconnect::{SupportedCard, supported_card};

fn init_nvml() -> Result<Nvml> {
    #[cfg(target_os = "linux")]
    let nvml = Nvml::builder().lib_path(std::ffi::OsStr::new("libnvidia-ml.so.1")).init();
    #[cfg(not(target_os = "linux"))]
    let nvml = Nvml::init();
    nvml.context("initializing NVML")
}

/// Identity and static data of one NVIDIA card, read once.
#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub name: String,
    /// PCI device ID as `device << 16 | vendor`.
    pub device_id: u32,
    /// PCI subsystem ID as `subsystem device << 16 | subsystem vendor` (0 if unknown).
    pub subsystem_id: u32,
    /// PCI address as NVML reports it, e.g. `00000000:0A:00.0`.
    pub pci_address: String,
    pub pci_bus: u32,
    pub vbios: Option<String>,
    pub memory_bytes: Option<u64>,
    /// The matching entry of the supported-cards list, if this card is supported.
    pub supported: Option<&'static SupportedCard>,
}

/// Driver version and all NVIDIA cards in this PC.
#[derive(Clone, Debug)]
pub struct System {
    pub driver_version: Option<String>,
    pub gpus: Vec<GpuInfo>,
}

/// Reads the identity of every NVIDIA card in this PC.
pub fn detect() -> Result<System> {
    let nvml = init_nvml()?;
    let mut gpus = Vec::new();
    for index in 0..nvml.device_count()? {
        let device = nvml.device_by_index(index)?;
        let pci = device.pci_info()?;
        let subsystem_id = pci.pci_sub_system_id.unwrap_or(0);
        gpus.push(GpuInfo {
            name: device.name().unwrap_or_else(|_| "Unknown NVIDIA GPU".into()),
            device_id: pci.pci_device_id,
            subsystem_id,
            pci_address: pci.bus_id.clone(),
            pci_bus: pci.bus,
            vbios: device.vbios_version().ok(),
            memory_bytes: device.memory_info().ok().map(|m| m.total),
            supported: supported_card(pci.pci_device_id, subsystem_id),
        });
    }
    Ok(System { driver_version: nvml.sys_driver_version().ok(), gpus })
}

pub struct Gpu {
    nvml: Nvml,
    index: u32,
}

impl Gpu {
    /// Initializes NVML and finds the supported card on PCI bus `pci_bus` (the card the fan
    /// controller was found on). With `None`, the first supported card is used.
    pub fn open(pci_bus: Option<u32>) -> Result<Self> {
        let nvml = init_nvml()?;
        for index in 0..nvml.device_count()? {
            let pci = nvml.device_by_index(index)?.pci_info()?;
            let supported = supported_card(pci.pci_device_id, pci.pci_sub_system_id.unwrap_or(0)).is_some();
            if supported && pci_bus.is_none_or(|bus| bus == pci.bus) {
                return Ok(Self { nvml, index });
            }
        }
        match pci_bus {
            Some(bus) => bail!("NVML: no supported graphics card on PCI bus {bus:02x}"),
            None => bail!("NVML: no supported graphics card found"),
        }
    }

    /// GPU core temperature in °C.
    pub fn temperature(&self) -> Result<u32> {
        Ok(self.nvml.device_by_index(self.index)?.temperature(TemperatureSensor::Gpu)?)
    }
}
