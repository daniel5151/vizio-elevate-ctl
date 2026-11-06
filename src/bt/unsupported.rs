//! Stub Bluetooth implementation for unsupported platforms.

use std::collections::HashMap;

use anyhow::Result;
use anyhow::bail;

use super::types::BleDeviceInfo;
use super::types::GattCharInfo;
use super::types::NotifyHandler;
use crate::Cli;

/// Scan BLE advertisements (unsupported).
pub fn scan(_secs: u64, _stop_on: Option<&str>) -> Result<HashMap<u64, (String, i16)>> {
    bail!("Bluetooth scanning is not yet implemented for this platform");
}

/// Paired devices (unsupported).
pub fn paired_devices() -> Result<Vec<(String, u64, &'static str)>> {
    bail!("Listing paired devices is not yet implemented for this platform");
}

/// Pair with soundbar (unsupported).
pub fn pair(_cli: &Cli) -> Result<()> {
    bail!("Bluetooth pairing is not yet implemented for this platform");
}

/// Remove pairing (unsupported).
pub fn unpair(_cli: &Cli) -> Result<()> {
    bail!("Bluetooth unpairing is not yet implemented for this platform");
}

/// Unsupported BLE connection.
pub struct BleConnection;

impl BleConnection {
    /// Connect (unsupported).
    pub fn connect(_cli: &Cli, _notify_handler: NotifyHandler) -> Result<Self> {
        bail!("Bluetooth LE connection is not yet implemented for this platform");
    }

    /// Read (unsupported).
    pub fn read(&self, _handle: u16) -> Result<Vec<u8>> {
        bail!("not implemented");
    }

    /// Write (unsupported).
    pub fn write(&self, _handle: u16, _data: &[u8], _with_response: Option<bool>) -> Result<()> {
        bail!("not implemented");
    }

    /// Subscribe (unsupported).
    pub fn subscribe(&mut self, _handle: u16) -> Result<()> {
        bail!("not implemented");
    }

    /// Characteristics (unsupported).
    pub fn characteristics(&self) -> &[GattCharInfo] {
        &[]
    }

    /// Device info (unsupported).
    pub fn device_info(&self) -> Result<BleDeviceInfo> {
        bail!("not implemented");
    }

    /// Handle offset (unsupported).
    pub fn handle_offset(&self) -> i32 {
        0
    }

    /// Close (unsupported).
    pub fn close(self) {}
}

/// Print platform troubleshooting hint (no-op).
pub fn print_platform_error_hint(_err: &anyhow::Error) {}
