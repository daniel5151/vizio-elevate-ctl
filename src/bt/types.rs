//! Platform-agnostic Bluetooth types.

use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use anyhow::bail;

/// Callback invoked when a characteristic notification or indication is received.
pub type NotifyHandler = Arc<dyn Fn(u16, Vec<u8>) + Send + Sync>;

/// 128-bit Bluetooth UUID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BleUuid {
    /// First 32 bits (contains 16-bit short UUID for standard Bluetooth SIG UUIDs).
    pub data1: u32,
    /// Next 16 bits.
    pub data2: u16,
    /// Next 16 bits.
    pub data3: u16,
    /// Final 64 bits.
    pub data4: [u8; 8],
}

impl BleUuid {
    /// Returns the 16-bit short UUID if this UUID is based on the Bluetooth SIG base UUID.
    pub fn short_uuid(&self) -> Option<u16> {
        const BASE_TAIL: [u8; 8] = [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb];
        (self.data1 <= 0xffff && self.data2 == 0 && self.data3 == 0x1000 && self.data4 == BASE_TAIL)
            .then_some(self.data1 as u16)
    }
}

impl fmt::Display for BleUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = self.short_uuid() {
            write!(f, "0x{s:04X}")
        } else {
            write!(
                f,
                "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                self.data1,
                self.data2,
                self.data3,
                self.data4[0],
                self.data4[1],
                self.data4[2],
                self.data4[3],
                self.data4[4],
                self.data4[5],
                self.data4[6],
                self.data4[7],
            )
        }
    }
}

impl fmt::Debug for BleUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// GATT characteristic properties bitflags.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct GattProps(pub u32);

impl GattProps {
    /// Broadcast.
    pub const BROADCAST: u32 = 1 << 0;
    /// Read.
    pub const READ: u32 = 1 << 1;
    /// Write without response.
    pub const WRITE_WITHOUT_RESPONSE: u32 = 1 << 2;
    /// Write with response.
    pub const WRITE: u32 = 1 << 3;
    /// Notify.
    pub const NOTIFY: u32 = 1 << 4;
    /// Indicate.
    pub const INDICATE: u32 = 1 << 5;

    /// Returns true if readable.
    pub fn can_read(&self) -> bool {
        self.0 & Self::READ != 0
    }

    /// Returns true if notifications or indications are supported.
    pub fn can_notify(&self) -> bool {
        self.0 & (Self::NOTIFY | Self::INDICATE) != 0
    }

    /// Returns true if write without response is supported.
    pub fn can_write_without_response(&self) -> bool {
        self.0 & Self::WRITE_WITHOUT_RESPONSE != 0
    }
}

impl fmt::Display for GattProps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = [
            (Self::BROADCAST, "bcast"),
            (Self::READ, "read"),
            (Self::WRITE_WITHOUT_RESPONSE, "write-nr"),
            (Self::WRITE, "write"),
            (Self::NOTIFY, "notify"),
            (Self::INDICATE, "indicate"),
        ];
        let active: Vec<_> = names
            .iter()
            .filter(|(flag, _)| self.0 & flag != 0)
            .map(|(_, name)| *name)
            .collect();
        write!(f, "{}", active.join(","))
    }
}

impl fmt::Debug for GattProps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// Platform-agnostic description of a GATT characteristic.
#[derive(Clone, Debug)]
pub struct GattCharInfo {
    /// ATT value handle.
    pub handle: u16,
    /// Characteristic UUID.
    pub uuid: BleUuid,
    /// Parent service UUID.
    pub service_uuid: BleUuid,
    /// Properties (read, write, notify, etc.).
    pub props: GattProps,
}

/// Summary information about a connected BLE device.
#[derive(Clone, Debug)]
pub struct BleDeviceInfo {
    /// Advertised or paired device name.
    pub name: String,
    /// 48-bit Bluetooth MAC address.
    pub address: u64,
    /// Whether the device is currently connected.
    pub connected: bool,
    /// Whether the device is paired at the OS level (if known).
    pub paired: Option<bool>,
}

/// Format a 48-bit Bluetooth address as `AA:BB:CC:DD:EE:FF`.
pub fn fmt_addr(a: u64) -> String {
    (0..6)
        .rev()
        .map(|i| format!("{:02X}", (a >> (i * 8)) & 0xff))
        .collect::<Vec<_>>()
        .join(":")
}

/// Parse a Bluetooth address string in hex form (with or without colons/dashes).
pub fn parse_addr(s: &str) -> Result<u64> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        bail!("invalid Bluetooth address {s:?}");
    }
    Ok(u64::from_str_radix(&hex, 16)?)
}
