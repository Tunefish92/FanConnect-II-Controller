//! Windows transport: NVIDIA's NVAPI (nvapi64.dll) I2C functions, called exactly as
//! GPU Tweak III was captured calling them (see captures/2026-09-25-gputweak).

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::null_mut;

use anyhow::{Context, Result, anyhow, bail};
use libloading::Library;

use crate::fanconnect::{ADDRESS, PORT, SupportedCard, Transport, format_pci_ids, supported_card};

type GpuHandle = *mut c_void;
type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type Initialize = unsafe extern "C" fn() -> i32;
type EnumPhysicalGpus = unsafe extern "C" fn(*mut GpuHandle, *mut u32) -> i32;
type GetPciIdentifiers = unsafe extern "C" fn(GpuHandle, *mut u32, *mut u32, *mut u32, *mut u32) -> i32;
type GetBusId = unsafe extern "C" fn(GpuHandle, *mut u32) -> i32;
type I2cTransfer = unsafe extern "C" fn(GpuHandle, *mut I2cInfo, *mut u32) -> i32;

const ID_INITIALIZE: u32 = 0x0150_E828;
const ID_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC_921F;
const ID_GET_PCI_IDENTIFIERS: u32 = 0x2DDF_B66E;
const ID_GET_BUS_ID: u32 = 0x1BE0_B8E5;
const ID_I2C_READ_EX: u32 = 0x4D7B_0709;
const ID_I2C_WRITE_EX: u32 = 0x283A_C65A;

const MAX_PHYSICAL_GPUS: usize = 64;
/// NV_I2C_SPEED value 4 (100 kHz), as used by GPU Tweak III.
const I2C_SPEED_KHZ: u32 = 4;
/// Marks the deprecated `i2cSpeed` field as unused.
const I2C_SPEED_DEPRECATED: u32 = 0xFFFF;

/// NV_I2C_INFO_V3 from nvapi.h.
#[repr(C)]
struct I2cInfo {
    version: u32,
    display_mask: u32,
    is_ddc_port: u8,
    /// 8-bit form: 7-bit address << 1.
    dev_address: u8,
    reg_address: *mut u8,
    reg_address_size: u32,
    data: *mut u8,
    size: u32,
    speed: u32,
    speed_khz: u32,
    port_id: u8,
    is_port_id_set: u32,
}

pub struct Nvapi {
    _lib: Library,
    gpu: GpuHandle,
    bus_id: u32,
    card: &'static SupportedCard,
    read_ex: I2cTransfer,
    write_ex: I2cTransfer,
}

fn resolve<T: Copy>(query: QueryInterface, id: u32, name: &str) -> Result<T> {
    // SAFETY: nvapi_QueryInterface returns a function pointer for a known ID, or null.
    let ptr = unsafe { query(id) };
    if ptr.is_null() {
        bail!("NVAPI function {name} not available in this driver");
    }
    // SAFETY: T is the function pointer type documented for this ID.
    Ok(unsafe { std::mem::transmute_copy(&ptr) })
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(anyhow!("{what} failed with NVAPI status {status}")) }
}

/// Loads NVAPI and finds the supported card.
pub fn open() -> Result<Nvapi> {
    // SAFETY: nvapi64.dll is NVIDIA's driver library; loading it runs no unexpected code.
    let lib = unsafe { Library::new("nvapi64.dll") }.context("loading nvapi64.dll (is the NVIDIA driver installed?)")?;
    // SAFETY: the symbol has this signature in every NVAPI version.
    let query: QueryInterface = *unsafe { lib.get::<QueryInterface>(b"nvapi_QueryInterface\0") }
        .context("nvapi_QueryInterface not found in nvapi64.dll")?;

    let initialize: Initialize = resolve(query, ID_INITIALIZE, "NvAPI_Initialize")?;
    let enum_gpus: EnumPhysicalGpus = resolve(query, ID_ENUM_PHYSICAL_GPUS, "NvAPI_EnumPhysicalGPUs")?;
    let pci_ids: GetPciIdentifiers = resolve(query, ID_GET_PCI_IDENTIFIERS, "NvAPI_GPU_GetPCIIdentifiers")?;
    let get_bus_id: GetBusId = resolve(query, ID_GET_BUS_ID, "NvAPI_GPU_GetBusId")?;
    let read_ex: I2cTransfer = resolve(query, ID_I2C_READ_EX, "NvAPI_I2CReadEx")?;
    let write_ex: I2cTransfer = resolve(query, ID_I2C_WRITE_EX, "NvAPI_I2CWriteEx")?;

    let mut unsupported = Vec::new();
    // SAFETY: plain NVAPI calls with valid out-pointers.
    unsafe {
        check(initialize(), "NvAPI_Initialize")?;
        let mut handles = [null_mut(); MAX_PHYSICAL_GPUS];
        let mut count = 0u32;
        check(enum_gpus(handles.as_mut_ptr(), &mut count), "NvAPI_EnumPhysicalGPUs")?;
        for &gpu in handles.iter().take(count as usize) {
            let (mut device, mut subsystem, mut revision, mut ext) = (0, 0, 0, 0);
            if pci_ids(gpu, &mut device, &mut subsystem, &mut revision, &mut ext) != 0 {
                continue;
            }
            match supported_card(device, subsystem) {
                Some(card) => {
                    let mut bus_id = 0;
                    check(get_bus_id(gpu, &mut bus_id), "NvAPI_GPU_GetBusId")?;
                    return Ok(Nvapi { _lib: lib, gpu, bus_id, card, read_ex, write_ex });
                }
                None => unsupported.push(format_pci_ids(device, subsystem)),
            }
        }
    }
    if unsupported.is_empty() {
        bail!("no NVIDIA graphics card found through NVAPI");
    }
    bail!("no supported graphics card found; not supported: {}", unsupported.join("; "))
}

impl Nvapi {
    fn transfer(&self, function: I2cTransfer, reg: u8, value: &mut u8) -> io::Result<()> {
        let mut reg_buf = [reg];
        let mut data = [*value];
        let mut unknown = 0u32;
        let mut info = I2cInfo {
            version: size_of::<I2cInfo>() as u32 | (3 << 16),
            display_mask: 0,
            is_ddc_port: 0,
            dev_address: ADDRESS << 1,
            reg_address: reg_buf.as_mut_ptr(),
            reg_address_size: 1,
            data: data.as_mut_ptr(),
            size: 1,
            speed: I2C_SPEED_DEPRECATED,
            speed_khz: I2C_SPEED_KHZ,
            port_id: PORT,
            is_port_id_set: 1,
        };
        // SAFETY: info and the buffers it points to outlive the call.
        let status = unsafe { function(self.gpu, &mut info, &mut unknown) };
        if status != 0 {
            return Err(io::Error::other(format!("NVAPI I2C status {status}")));
        }
        *value = data[0];
        Ok(())
    }
}

impl Transport for Nvapi {
    fn read(&self, reg: u8) -> io::Result<u8> {
        let mut value = 0;
        self.transfer(self.read_ex, reg, &mut value)?;
        Ok(value)
    }

    fn write(&self, reg: u8, value: u8) -> io::Result<()> {
        let mut value = value;
        self.transfer(self.write_ex, reg, &mut value)
    }

    fn describe(&self) -> String {
        format!("NVAPI I2C port {PORT}, GPU bus {:02x}", self.bus_id)
    }

    fn pci_bus(&self) -> u32 {
        self.bus_id
    }

    fn card(&self) -> &'static SupportedCard {
        self.card
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i2c_info_matches_nvapi_v3_layout() {
        // NV_I2C_INFO_V3 on x64: pointers at offsets 16 and 32, 64 bytes in total.
        assert_eq!(size_of::<I2cInfo>(), 64);
        assert_eq!(std::mem::offset_of!(I2cInfo, reg_address), 16);
        assert_eq!(std::mem::offset_of!(I2cInfo, data), 32);
        assert_eq!(std::mem::offset_of!(I2cInfo, port_id), 52);
        assert_eq!(std::mem::offset_of!(I2cInfo, is_port_id_set), 56);
    }
}
