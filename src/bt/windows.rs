//! Windows implementation of Bluetooth LE hardware abstraction layer using WinRT.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use windows::Devices::Bluetooth::Advertisement::BluetoothLEAdvertisementReceivedEventArgs;
use windows::Devices::Bluetooth::Advertisement::BluetoothLEAdvertisementWatcher;
use windows::Devices::Bluetooth::Advertisement::BluetoothLEScanningMode;
use windows::Devices::Bluetooth::BluetoothCacheMode;
use windows::Devices::Bluetooth::BluetoothConnectionStatus;
use windows::Devices::Bluetooth::BluetoothDevice;
use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattCharacteristic;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattCharacteristicProperties;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattClientCharacteristicConfigurationDescriptorValue;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattCommunicationStatus;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattDeviceService;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattSession;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattSharingMode;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattValueChangedEventArgs;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattWriteOption;
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Enumeration::DeviceInformationCustomPairing;
use windows::Devices::Enumeration::DevicePairingKinds;
use windows::Devices::Enumeration::DevicePairingRequestedEventArgs;
use windows::Devices::Enumeration::DevicePairingResultStatus;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataReader;
use windows::Storage::Streams::DataWriter;
use windows::Storage::Streams::IBuffer;
use windows::core::GUID;
use windows::core::HSTRING;

use super::types::BleDeviceInfo;
use super::types::BleUuid;
use super::types::GattCharInfo;
use super::types::GattProps;
use super::types::NotifyHandler;
use super::types::fmt_addr;
use super::types::parse_addr;
use crate::Cli;
use crate::KNOWN_HANDLES;

fn buf_to_vec(buf: &IBuffer) -> Result<Vec<u8>> {
    let len = buf.Length()? as usize;
    let mut v = vec![0u8; len];
    if len > 0 {
        DataReader::FromBuffer(buf)?.ReadBytes(&mut v)?;
    }
    Ok(v)
}

fn vec_to_buf(data: &[u8]) -> Result<IBuffer> {
    let w = DataWriter::new()?;
    w.WriteBytes(data)?;
    Ok(w.DetachBuffer()?)
}

fn convert_guid(g: GUID) -> BleUuid {
    BleUuid {
        data1: g.data1,
        data2: g.data2,
        data3: g.data3,
        data4: g.data4,
    }
}

fn convert_props(p: u32) -> GattProps {
    let mut flags = 0;
    if p & GattCharacteristicProperties::Broadcast.0 != 0 {
        flags |= GattProps::BROADCAST;
    }
    if p & GattCharacteristicProperties::Read.0 != 0 {
        flags |= GattProps::READ;
    }
    if p & GattCharacteristicProperties::WriteWithoutResponse.0 != 0 {
        flags |= GattProps::WRITE_WITHOUT_RESPONSE;
    }
    if p & GattCharacteristicProperties::Write.0 != 0 {
        flags |= GattProps::WRITE;
    }
    if p & GattCharacteristicProperties::Notify.0 != 0 {
        flags |= GattProps::NOTIFY;
    }
    if p & GattCharacteristicProperties::Indicate.0 != 0 {
        flags |= GattProps::INDICATE;
    }
    GattProps(flags)
}

fn config_file() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("vizio-ctl.toml")))
}

fn load_cached_addr() -> Option<u64> {
    let s = std::fs::read_to_string(config_file()?).ok()?;
    let table: toml::Table = s.parse().ok()?;
    let addr_str = table.get("address")?.as_str()?;
    parse_addr(addr_str.trim()).ok()
}

fn save_cached_addr(a: u64) {
    if let Some(f) = config_file() {
        let mut table: toml::Table = std::fs::read_to_string(&f)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        table.insert("address".to_string(), toml::Value::String(fmt_addr(a)));
        if let Some(p) = f.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(f, table.to_string());
    }
}

/// Scan BLE advertisements. Returns address -> (name, rssi).
pub fn scan(secs: u64, stop_on: Option<&str>) -> Result<HashMap<u64, (String, i16)>> {
    let seen: Arc<Mutex<HashMap<u64, (String, i16)>>> = Arc::default();
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;
    let s2 = seen.clone();
    watcher.Received(&TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementReceivedEventArgs,
    >::new(move |_, args| {
        if let Some(a) = args.as_ref() {
            let addr = a.BluetoothAddress()?;
            let rssi = a.RawSignalStrengthInDBm()?;
            let name = a.Advertisement()?.LocalName()?.to_string();
            let mut m = s2.lock().unwrap();
            let e = m.entry(addr).or_insert((String::new(), rssi));
            e.1 = rssi;
            if !name.is_empty() {
                e.0 = name;
            }
        }
        Ok(())
    }))?;
    watcher.Start()?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    let needle = stop_on.map(|s| s.to_lowercase());
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        if let Some(n) = &needle {
            if seen
                .lock()
                .unwrap()
                .values()
                .any(|(name, _)| name.to_lowercase().contains(n))
            {
                break;
            }
        }
    }
    watcher.Stop()?;
    let m = seen.lock().unwrap().clone();
    Ok(m)
}

/// Paired devices (LE and classic) as (name, address, kind).
pub fn paired_devices() -> Result<Vec<(String, u64, &'static str)>> {
    let mut out = Vec::new();
    let le = DeviceInformation::FindAllAsyncAqsFilter(
        &BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?,
    )?
    .join()?;
    for di in &le {
        if let Ok(d) = BluetoothLEDevice::FromIdAsync(&di.Id()?)?.join() {
            out.push((di.Name()?.to_string(), d.BluetoothAddress()?, "LE"));
            let _ = d.Close();
        }
    }
    let classic = DeviceInformation::FindAllAsyncAqsFilter(
        &BluetoothDevice::GetDeviceSelectorFromPairingState(true)?,
    )?
    .join()?;
    for di in &classic {
        if let Ok(d) = BluetoothDevice::FromIdAsync(&di.Id()?)?.join() {
            out.push((di.Name()?.to_string(), d.BluetoothAddress()?, "classic"));
            let _ = d.Close();
        }
    }
    Ok(out)
}

fn le_from_addr(a: u64) -> Result<BluetoothLEDevice> {
    BluetoothLEDevice::FromBluetoothAddressAsync(a)?
        .join()
        .map_err(|e| anyhow!("no BLE device at {}: {e}", fmt_addr(a)))
}

fn find_device(cli: &Cli) -> Result<BluetoothLEDevice> {
    if let Some(a) = &cli.address {
        return le_from_addr(parse_addr(a)?);
    }
    if let Some(a) = load_cached_addr() {
        if let Ok(d) = le_from_addr(a) {
            eprintln!("Using remembered device {}", fmt_addr(a));
            return Ok(d);
        }
    }
    let needle = cli.name.to_lowercase();
    if let Ok(paired) = paired_devices() {
        for (name, addr, kind) in paired {
            if name.to_lowercase().contains(&needle) {
                if let Ok(d) = le_from_addr(addr) {
                    eprintln!("Using paired ({kind}) device {name:?} {}", fmt_addr(addr));
                    return Ok(d);
                }
            }
        }
    }
    eprintln!(
        "Scanning up to {}s for a BLE device named *{}*...",
        cli.scan_secs, cli.name
    );
    let seen = scan(cli.scan_secs, Some(&cli.name))?;
    let best = seen
        .iter()
        .filter(|(_, (n, _))| n.to_lowercase().contains(&needle))
        .max_by_key(|(_, (_, rssi))| *rssi);
    match best {
        Some((addr, (name, rssi))) => {
            eprintln!("Found {name:?} {} ({rssi} dBm)", fmt_addr(*addr));
            le_from_addr(*addr)
        }
        None => bail!(
            "soundbar not found. Make sure it's on, close the VIZIO app / turn off Bluetooth on \
             your phone (BLE peripherals often accept only one connection), or pass --address"
        ),
    }
}

/// Pair with the soundbar at the OS level.
pub fn pair(cli: &Cli) -> Result<()> {
    let device = find_device(cli)?;
    let addr = device.BluetoothAddress()?;
    let pairing = device.DeviceInformation()?.Pairing()?;
    if pairing.IsPaired()? {
        println!("{} is already paired.", fmt_addr(addr));
        save_cached_addr(addr);
        return Ok(());
    }
    println!("Pairing with {} ({})...", device.Name()?, fmt_addr(addr));
    let custom: DeviceInformationCustomPairing = pairing.Custom()?;
    custom.PairingRequested(&TypedEventHandler::<
        DeviceInformationCustomPairing,
        DevicePairingRequestedEventArgs,
    >::new(|_, args| {
        let Some(a) = args.as_ref() else {
            return Ok(());
        };
        let kind = a.PairingKind()?;
        if kind == DevicePairingKinds::ConfirmOnly {
            a.Accept()?;
        } else if kind == DevicePairingKinds::ConfirmPinMatch
            || kind == DevicePairingKinds::DisplayPin
        {
            println!("PIN: {} (accepting)", a.Pin()?);
            a.Accept()?;
        } else if kind == DevicePairingKinds::ProvidePin {
            use std::io::Write as _;
            print!("Enter PIN: ");
            let _ = std::io::stdout().flush();
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            a.AcceptWithPin(&HSTRING::from(s.trim()))?;
        } else {
            a.Accept()?;
        }
        Ok(())
    }))?;
    let kinds = DevicePairingKinds(
        DevicePairingKinds::ConfirmOnly.0
            | DevicePairingKinds::ConfirmPinMatch.0
            | DevicePairingKinds::DisplayPin.0
            | DevicePairingKinds::ProvidePin.0,
    );
    let status = custom.PairAsync(kinds)?.join()?.Status()?;
    if status == DevicePairingResultStatus::Paired
        || status == DevicePairingResultStatus::AlreadyPaired
    {
        println!("Paired.");
        save_cached_addr(addr);
        Ok(())
    } else {
        bail!("pairing failed: {status:?}")
    }
}

/// Remove OS-level pairing.
pub fn unpair(cli: &Cli) -> Result<()> {
    let device = find_device(cli)?;
    let st = device
        .DeviceInformation()?
        .Pairing()?
        .UnpairAsync()?
        .join()?
        .Status()?;
    println!("Unpair: {st:?}");
    Ok(())
}

struct WinCharInfo {
    ch: GattCharacteristic,
    attr_handle: u16,
    uuid: BleUuid,
    service_uuid: BleUuid,
    props: GattProps,
}

/// Windows Bluetooth LE connection.
pub struct BleConnection {
    device: BluetoothLEDevice,
    session: Option<GattSession>,
    services: Vec<GattDeviceService>,
    chars: Vec<WinCharInfo>,
    cached_chars: Vec<GattCharInfo>,
    offset: i32,
    subscribed: Vec<u16>,
    notify_handler: NotifyHandler,
    verbose: bool,
}

impl BleConnection {
    /// Connect to the device and enumerate its GATT services and characteristics.
    pub fn connect(cli: &Cli, notify_handler: NotifyHandler) -> Result<Self> {
        let device = find_device(cli)?;
        let addr = device.BluetoothAddress()?;
        eprintln!("Connecting to {} ({})...", device.Name()?, fmt_addr(addr));

        let session = GattSession::FromDeviceIdAsync(&device.BluetoothDeviceId()?)
            .and_then(|op| op.join())
            .ok();
        if let Some(s) = &session {
            let _ = s.SetMaintainConnection(true);
        }

        let mut services = Vec::new();
        let mut last_err = String::new();
        for attempt in 1..=3 {
            let r = device
                .GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached)?
                .join()?;
            if r.Status()? == GattCommunicationStatus::Success {
                services = r.Services()?.into_iter().collect();
                break;
            }
            last_err = format!("{:?}", r.Status()?);
            eprintln!("  GATT discovery attempt {attempt} failed ({last_err}), retrying...");
            std::thread::sleep(Duration::from_millis(800));
        }
        if services.is_empty() {
            bail!(
                "could not discover GATT services ({last_err}). Is the soundbar on and not connected to your phone?"
            );
        }

        let mut chars = Vec::new();
        let mut denied = 0;
        for svc in &services {
            // Open shared so several vizio-ctl instances (or other apps) can coexist.
            let _ = svc
                .OpenAsync(GattSharingMode::SharedReadAndWrite)
                .and_then(|op| op.join());
            let r = svc
                .GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached)?
                .join()?;
            if r.Status()? != GattCommunicationStatus::Success {
                if r.Status()? == GattCommunicationStatus::AccessDenied {
                    denied += 1;
                }
                if cli.verbose {
                    eprintln!(
                        "  service {} characteristics: {:?}",
                        convert_guid(svc.Uuid()?),
                        r.Status()?
                    );
                }
                continue;
            }
            let service_uuid = convert_guid(svc.Uuid()?);
            for ch in r.Characteristics()? {
                chars.push(WinCharInfo {
                    attr_handle: ch.AttributeHandle()?,
                    uuid: convert_guid(ch.Uuid()?),
                    props: convert_props(ch.CharacteristicProperties()?.0),
                    service_uuid,
                    ch,
                });
            }
        }

        // Windows may report either the characteristic declaration handle or
        // the value handle; pick whichever offset matches the handles
        // from the capture.
        let offset = [0i32, 1, -1]
            .into_iter()
            .max_by_key(|off| {
                KNOWN_HANDLES
                    .iter()
                    .filter(|&&h| chars.iter().any(|c| c.attr_handle as i32 + off == h as i32))
                    .count()
            })
            .unwrap();
        let matched = KNOWN_HANDLES
            .iter()
            .filter(|&&h| {
                chars
                    .iter()
                    .any(|c| c.attr_handle as i32 + offset == h as i32)
            })
            .count();
        if cli.verbose {
            eprintln!(
                "  {} services, {} characteristics, handle offset {offset:+}, {matched}/{} known handles matched",
                services.len(),
                chars.len(),
                KNOWN_HANDLES.len()
            );
        }
        if matched < 3 {
            if denied > 0 {
                bail!(
                    "access denied to {denied} GATT service(s) - another program (e.g. an older \
                     `vizio-ctl shell` still running) has the soundbar open exclusively. Close it and retry."
                );
            }
            eprintln!(
                "WARNING: GATT layout doesn't match the capture ({matched}/{} handles). \
                 Run `dump` to inspect.",
                KNOWN_HANDLES.len()
            );
        }

        save_cached_addr(addr);

        let cached_chars = chars
            .iter()
            .map(|c| GattCharInfo {
                handle: (c.attr_handle as i32 + offset) as u16,
                uuid: c.uuid,
                service_uuid: c.service_uuid,
                props: c.props,
            })
            .collect();

        Ok(Self {
            device,
            session,
            services,
            chars,
            cached_chars,
            offset,
            subscribed: Vec::new(),
            notify_handler,
            verbose: cli.verbose,
        })
    }

    fn value_handle(&self, c: &WinCharInfo) -> u16 {
        (c.attr_handle as i32 + self.offset) as u16
    }

    fn char_for(&self, handle: u16) -> Result<&WinCharInfo> {
        self.chars
            .iter()
            .find(|c| self.value_handle(c) == handle)
            .ok_or_else(|| anyhow!("no characteristic with handle 0x{handle:04x}"))
    }

    /// Read raw bytes from characteristic by ATT value handle.
    pub fn read(&self, handle: u16) -> Result<Vec<u8>> {
        let c = self.char_for(handle)?;
        let r =
            c.ch.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?
                .join()?;
        let st = r.Status()?;
        if st != GattCommunicationStatus::Success {
            let pe = r.ProtocolError().and_then(|p| p.Value()).ok();
            bail!("read 0x{handle:04x} failed: {st:?} (ATT error {pe:?})");
        }
        let v = buf_to_vec(&r.Value()?)?;
        if self.verbose {
            eprintln!(
                "  read  0x{handle:04x} -> {} \"{}\"",
                crate::hex(&v),
                crate::printable(&v)
            );
        }
        Ok(v)
    }

    /// Write raw bytes to characteristic by ATT value handle.
    pub fn write(&self, handle: u16, data: &[u8], with_response: Option<bool>) -> Result<()> {
        let c = self.char_for(handle)?;
        let with_response = with_response.unwrap_or(!c.props.can_write_without_response());
        let opt = if with_response {
            GattWriteOption::WriteWithResponse
        } else {
            GattWriteOption::WriteWithoutResponse
        };
        if self.verbose {
            eprintln!(
                "  write 0x{handle:04x} <- {} \"{}\" ({})",
                crate::hex(data),
                crate::printable(data),
                if with_response { "req" } else { "cmd" }
            );
        }
        let st =
            c.ch.WriteValueWithOptionAsync(&vec_to_buf(data)?, opt)?
                .join()?;
        if st != GattCommunicationStatus::Success {
            bail!("write 0x{handle:04x} failed: {st:?}");
        }
        Ok(())
    }

    /// Subscribe to notifications or indications on a characteristic.
    pub fn subscribe(&mut self, handle: u16) -> Result<()> {
        if self.subscribed.contains(&handle) {
            return Ok(());
        }
        let c = self.char_for(handle)?;
        let cccd = if c.props.0 & GattProps::NOTIFY != 0 {
            GattClientCharacteristicConfigurationDescriptorValue::Notify
        } else if c.props.0 & GattProps::INDICATE != 0 {
            GattClientCharacteristicConfigurationDescriptorValue::Indicate
        } else {
            bail!("0x{handle:04x} does not support notifications");
        };
        let handler = self.notify_handler.clone();
        c.ch.ValueChanged(&TypedEventHandler::<
            GattCharacteristic,
            GattValueChangedEventArgs,
        >::new(move |_, args| {
            if let Some(a) = args.as_ref() {
                if let Ok(v) = buf_to_vec(&a.CharacteristicValue()?) {
                    handler(handle, v);
                }
            }
            Ok(())
        }))?;
        let st =
            c.ch.WriteClientCharacteristicConfigurationDescriptorAsync(cccd)?
                .join()?;
        if st != GattCommunicationStatus::Success {
            bail!("subscribe 0x{handle:04x} failed: {st:?}");
        }
        self.subscribed.push(handle);
        Ok(())
    }

    /// Returns the list of discovered GATT characteristics.
    pub fn characteristics(&self) -> &[GattCharInfo] {
        &self.cached_chars
    }

    /// Returns device name, address, and connection state.
    pub fn device_info(&self) -> Result<BleDeviceInfo> {
        let address = self.device.BluetoothAddress()?;
        let name = self.device.Name()?.to_string();
        let connected = self.device.ConnectionStatus()? == BluetoothConnectionStatus::Connected;
        let paired = self
            .device
            .DeviceInformation()
            .and_then(|d| d.Pairing())
            .and_then(|p| p.IsPaired())
            .ok();
        Ok(BleDeviceInfo {
            name,
            address,
            connected,
            paired,
        })
    }

    /// Returns true if the BLE device is currently connected at the OS level.
    pub fn is_connected(&self) -> bool {
        self.device
            .ConnectionStatus()
            .map(|s| s == BluetoothConnectionStatus::Connected)
            .unwrap_or(false)
    }

    /// Returns handle offset between Windows AttributeHandle and ATT value handle.
    pub fn handle_offset(&self) -> i32 {
        self.offset
    }

    /// Close GATT services, session, and device connection.
    pub fn close(&self) {
        for s in &self.services {
            let _ = s.Close();
        }
        if let Some(s) = &self.session {
            let _ = s.SetMaintainConnection(false);
            let _ = s.Close();
        }
        let _ = self.device.Close();
    }
}

impl Drop for BleConnection {
    fn drop(&mut self) {
        self.close();
    }
}

/// Print platform-specific troubleshooting hints for an error.
pub fn print_platform_error_hint(err: &anyhow::Error) {
    if format!("{err:#}").contains("0x800710DF") {
        eprintln!(
            "hint: Bluetooth looks turned off - enable it in Windows Settings > Bluetooth & devices."
        );
    }
}
