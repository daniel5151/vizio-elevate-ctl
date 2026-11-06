//! vizio-ctl: control a VIZIO Elevate soundbar over Bluetooth LE

use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;
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

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

const H_INPUT_GUESS: u16 = 0x0052;
const H_DEVICE_NAME: u16 = 0x0054;
const H_DEVICE_TAG: u16 = 0x0056;
const H_MUTE: u16 = 0x006a;
const H_EQ: u16 = 0x0076;
const H_VOLUME: u16 = 0x0079;
const H_MODE_7C: u16 = 0x007c;
const H_STATE_82: u16 = 0x0082;
const H_AUTH_STATUS: u16 = 0x00bf;
const H_AUTH_CHALLENGE: u16 = 0x00c2;
const H_AUTH_RESPONSE: u16 = 0x00c5;

/// Handles used to auto-detect how Windows' `AttributeHandle` maps to ATT value
/// handles.
const KNOWN_HANDLES: &[u16] = &[
    H_INPUT_GUESS,
    H_DEVICE_NAME,
    H_DEVICE_TAG,
    H_MUTE,
    H_EQ,
    H_VOLUME,
    H_MODE_7C,
    H_STATE_82,
    H_AUTH_STATUS,
    H_AUTH_CHALLENGE,
    H_AUTH_RESPONSE,
];

const CLIENT_ID: &[u8] = b"1:f4c46a1e-4e22-";
const AUTH_SUFFIX: [u8; 14] = [
    0xf8, 0xb8, 0x18, 0x40, 0xc8, 0x20, 0x98, 0xe0, 0x08, 0x08, 0x20, 0x90, 0x38, 0x00,
];

const MAX_VOLUME: u8 = 100;

/// Characteristics the app polls after authenticating.
const INFO_FIELDS: &[(u16, &str, Fmt)] = &[
    (H_DEVICE_NAME, "Device name", Fmt::Str),
    (H_DEVICE_TAG, "Device tag", Fmt::Str),
    (H_VOLUME, "Volume", Fmt::U8),
    (H_MUTE, "Mute", Fmt::Bool),
    (H_EQ, "EQ Mode", Fmt::Eq),
    (H_STATE_82, "0x0082 (unknown)", Fmt::U8),
    (H_INPUT_GUESS, "0x0052 (unknown, maybe input)", Fmt::U8),
    (H_MODE_7C, "0x007c (unknown)", Fmt::U8),
    (0x0085, "0x0085 (unknown)", Fmt::U8),
    (0x0064, "0x0064 (unknown)", Fmt::U8),
    (0x007f, "0x007f (unknown)", Fmt::U8),
    (0x00ae, "0x00ae (unknown)", Fmt::U8),
    (0x006d, "0x006d (unknown)", Fmt::U8),
];

#[derive(Clone, Copy)]
enum Fmt {
    Str,
    U8,
    Bool,
    Eq,
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(version, about = "Control a VIZIO Elevate soundbar over Bluetooth LE")]
struct Cli {
    /// BLE address of the soundbar (AA:BB:CC:DD:EE:FF). Remembered after first
    /// success.
    #[arg(short, long, global = true)]
    address: Option<String>,

    /// Substring of the advertised/paired device name to look for.
    #[arg(short, long, global = true, default_value = "VIZIO")]
    name: String,

    /// How long to scan for the soundbar when it isn't already known.
    #[arg(long, global = true, default_value_t = 10)]
    scan_secs: u64,

    /// Print every GATT operation.
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Never raise the volume above this level.
    #[arg(long, global = true, default_value_t = MAX_VOLUME)]
    max_volume: u8,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan for nearby BLE devices and list paired ones.
    Scan,
    /// Pair (bond) with the soundbar at the OS level.
    Pair,
    /// Remove the OS-level pairing.
    Unpair,
    /// Interactive shell that keeps the connection open (fast volume changes).
    Shell,
    #[command(flatten)]
    Op(DeviceCmd),
}

#[derive(Subcommand, Clone)]
enum DeviceCmd {
    /// Connect, authenticate and print device info / state.
    Info,
    /// List every GATT service/characteristic with handles (and values if
    /// readable).
    Dump,
    /// Get or change the volume.
    #[command(alias = "vol", alias = "v")]
    Volume {
        #[command(subcommand)]
        action: Option<VolAction>,
    },
    /// Get, toggle, or set mute.
    #[command(alias = "m")]
    Mute {
        #[command(subcommand)]
        action: Option<MuteAction>,
    },
    /// Get or set the EQ mode.
    Eq {
        /// Mode to set (movie, music, game, direct). Omit to print current
        /// mode.
        mode: Option<EqMode>,
    },
    /// Read a raw characteristic by ATT value handle (e.g. 0x0079).
    Read { handle: String },
    /// Write raw hex bytes to a handle. The session token prefix "<token>:" is
    /// added unless --no-token.
    Write {
        handle: String,
        #[arg(required = true, num_args = 1..)]
        hex: Vec<String>,
        #[arg(long)]
        no_token: bool,
        #[arg(long)]
        with_response: bool,
    },
    /// Authenticate and print all notifications until Ctrl+C.
    Monitor,
}

#[derive(Parser)]
#[command(name = "vizio", no_binary_name = true)]
enum ShellCmd {
    #[command(flatten)]
    Device(DeviceCmd),

    /// Raise volume by N steps.
    #[command(alias = "+")]
    Up {
        /// Number of steps to raise.
        #[arg(default_value_t = 1)]
        steps: u8,
    },
    /// Lower volume by N steps.
    Down {
        /// Number of steps to lower.
        #[arg(default_value_t = 1)]
        steps: u8,
    },
    /// Set volume to an absolute level.
    Set {
        /// Volume level (0-100).
        level: u8,
    },
    /// Print current volume.
    Get,

    /// Save current state as baseline for diffing.
    #[command(alias = "snapshot")]
    Snap,
    /// Compare current state against baseline snapshot.
    #[command(alias = "d")]
    Diff {
        /// Diff all handles instead of just info fields.
        #[arg(long, short)]
        all: bool,
        /// Optional target (e.g. "all").
        target: Option<String>,
    },
    /// Subscribe to all notifications and print them live at prompt.
    Watch,
    /// Exit the interactive shell.
    #[command(alias = "q", alias = "exit")]
    Quit,
}

#[derive(Subcommand, Clone)]
enum VolAction {
    /// Print current volume.
    Get,
    /// Set an absolute volume level.
    Set { level: u8 },
    /// Raise volume by N steps (default 1).
    Up {
        #[arg(default_value_t = 1)]
        steps: u8,
    },
    /// Lower volume by N steps (default 1).
    Down {
        #[arg(default_value_t = 1)]
        steps: u8,
    },
}

#[derive(Subcommand, Clone)]
enum MuteAction {
    /// Print current mute state.
    #[command(alias = "status")]
    Get,
    /// Mute audio.
    #[command(alias = "1")]
    On,
    /// Unmute audio.
    #[command(alias = "0")]
    Off,
    /// Toggle mute state (default).
    #[command(alias = "t")]
    Toggle,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum EqMode {
    /// Movie EQ preset.
    Movie = 0,
    /// Music EQ preset.
    Music = 1,
    /// Game EQ preset.
    Game = 2,
    /// Direct EQ preset (unprocessed source audio).
    Direct = 3,
}

impl EqMode {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Movie),
            1 => Some(Self::Music),
            2 => Some(Self::Game),
            3 => Some(Self::Direct),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Movie => "Movie",
            Self::Music => "Music",
            Self::Game => "Game",
            Self::Direct => "Direct",
        }
    }
}

impl std::fmt::Display for EqMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for EqMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "0" | "movie" => Ok(Self::Movie),
            "1" | "music" => Ok(Self::Music),
            "2" | "game" => Ok(Self::Game),
            "3" | "direct" => Ok(Self::Direct),
            _ => bail!("unknown EQ mode {s:?} (expected movie, music, game, or direct)"),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_addr(s: &str) -> Result<u64> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        bail!("invalid Bluetooth address {s:?}");
    }
    Ok(u64::from_str_radix(&hex, 16)?)
}

fn fmt_addr(a: u64) -> String {
    (0..6)
        .rev()
        .map(|i| format!("{:02X}", (a >> (i * 8)) & 0xff))
        .collect::<Vec<_>>()
        .join(":")
}

fn parse_handle(s: &str) -> Result<u16> {
    let s = s.trim();
    Ok(
        if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            u16::from_str_radix(h, 16)?
        } else {
            s.parse()?
        },
    )
}

fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let hex: String = s
        .trim_start_matches("0x")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':' && *c != '-')
        .collect();
    if !hex.len().is_multiple_of(2) {
        bail!("hex string must have an even number of digits");
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(Into::into))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn printable(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}

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

fn short_uuid(g: &GUID) -> Option<u16> {
    let base = [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb];
    (g.data1 <= 0xffff && g.data2 == 0 && g.data3 == 0x1000 && g.data4 == base)
        .then_some(g.data1 as u16)
}

fn uuid_str(g: &GUID) -> String {
    match short_uuid(g) {
        Some(s) => format!("0x{s:04X}"),
        None => format!("{g:?}"),
    }
}

fn props_str(p: u32) -> String {
    let names = [
        (GattCharacteristicProperties::Broadcast.0, "bcast"),
        (GattCharacteristicProperties::Read.0, "read"),
        (
            GattCharacteristicProperties::WriteWithoutResponse.0,
            "write-nr",
        ),
        (GattCharacteristicProperties::Write.0, "write"),
        (GattCharacteristicProperties::Notify.0, "notify"),
        (GattCharacteristicProperties::Indicate.0, "indicate"),
    ];
    names
        .iter()
        .filter(|(f, _)| p & f != 0)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join(",")
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

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Scan BLE advertisements. Returns address -> (name, rssi).
fn scan(secs: u64, stop_on: Option<&str>) -> Result<HashMap<u64, (String, i16)>> {
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

/// Paired devices (LE and classic) as (name, address).
fn paired_devices() -> Result<Vec<(String, u64, &'static str)>> {
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

// ---------------------------------------------------------------------------
// Soundbar connection
// ---------------------------------------------------------------------------

struct CharInfo {
    ch: GattCharacteristic,
    attr_handle: u16,
    uuid: GUID,
    service_uuid: GUID,
    props: u32,
}

struct Soundbar {
    device: BluetoothLEDevice,
    session: Option<GattSession>,
    services: Vec<GattDeviceService>,
    chars: Vec<CharInfo>,
    /// value_handle = attr_handle + offset
    offset: i32,
    tx: Sender<(u16, Vec<u8>)>,
    rx: Receiver<(u16, Vec<u8>)>,
    subscribed: Vec<u16>,
    token: Option<Vec<u8>>,
    verbose: bool,
    max_volume: u8,
    /// Last volume level seen (via read or notification), used to de-duplicate.
    last_volume: Arc<Mutex<Option<u8>>>,
    /// When set (shell idle at the prompt), volume changes are printed
    /// immediately.
    live_print: Arc<AtomicBool>,
}

impl Soundbar {
    fn connect(cli: &Cli) -> Result<Self> {
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
            // Open shared so several vizio-ctl instances (or other apps) can
            // coexist.
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
                        uuid_str(&svc.Uuid()?),
                        r.Status()?
                    );
                }
                continue;
            }
            let service_uuid = svc.Uuid()?;
            for ch in r.Characteristics()? {
                chars.push(CharInfo {
                    attr_handle: ch.AttributeHandle()?,
                    uuid: ch.Uuid()?,
                    props: ch.CharacteristicProperties()?.0,
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
        let (tx, rx) = channel();
        Ok(Self {
            device,
            session,
            services,
            chars,
            offset,
            tx,
            rx,
            subscribed: Vec::new(),
            token: None,
            verbose: cli.verbose,
            max_volume: cli.max_volume.min(MAX_VOLUME),
            last_volume: Arc::default(),
            live_print: Arc::default(),
        })
    }

    fn value_handle(&self, c: &CharInfo) -> u16 {
        (c.attr_handle as i32 + self.offset) as u16
    }

    fn char_for(&self, handle: u16) -> Result<&CharInfo> {
        self.chars
            .iter()
            .find(|c| self.value_handle(c) == handle)
            .ok_or_else(|| anyhow!("no characteristic with handle 0x{handle:04x}"))
    }

    fn read(&self, handle: u16) -> Result<Vec<u8>> {
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
                hex(&v),
                printable(&v)
            );
        }
        Ok(v)
    }

    fn write_raw(&self, handle: u16, data: &[u8], with_response: Option<bool>) -> Result<()> {
        let c = self.char_for(handle)?;
        let with_response = with_response
            .unwrap_or(c.props & GattCharacteristicProperties::WriteWithoutResponse.0 == 0);
        let opt = if with_response {
            GattWriteOption::WriteWithResponse
        } else {
            GattWriteOption::WriteWithoutResponse
        };
        if self.verbose {
            eprintln!(
                "  write 0x{handle:04x} <- {} \"{}\" ({})",
                hex(data),
                printable(data),
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

    /// Write `"<token>:" + payload` like the app does for every control.
    fn write_cmd(&self, handle: u16, payload: &[u8]) -> Result<()> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow!("not authenticated"))?;
        let mut data = token.clone();
        data.push(b':');
        data.extend_from_slice(payload);
        self.write_raw(handle, &data, None)
    }

    fn subscribe(&mut self, handle: u16) -> Result<()> {
        if self.subscribed.contains(&handle) {
            return Ok(());
        }
        let c = self.char_for(handle)?;
        let cccd = if c.props & GattCharacteristicProperties::Notify.0 != 0 {
            GattClientCharacteristicConfigurationDescriptorValue::Notify
        } else if c.props & GattCharacteristicProperties::Indicate.0 != 0 {
            GattClientCharacteristicConfigurationDescriptorValue::Indicate
        } else {
            bail!("0x{handle:04x} does not support notifications");
        };
        let tx = self.tx.clone();
        let last_volume = self.last_volume.clone();
        let live_print = self.live_print.clone();
        c.ch.ValueChanged(&TypedEventHandler::<
            GattCharacteristic,
            GattValueChangedEventArgs,
        >::new(move |_, args| {
            if let Some(a) = args.as_ref() {
                if let Ok(v) = buf_to_vec(&a.CharacteristicValue()?) {
                    if handle == H_VOLUME && v.len() == 1 {
                        let mut last = last_volume.lock().unwrap();
                        let changed = *last != Some(v[0]);
                        *last = Some(v[0]);
                        if changed && live_print.load(Ordering::SeqCst) {
                            print!("\r[volume changed to {}]\nvizio> ", v[0]);
                            let _ = std::io::stdout().flush();
                        }
                    } else if handle == H_MUTE && v.len() == 1 {
                        if live_print.load(Ordering::SeqCst) {
                            let s = if v[0] != 0 { "on (muted)" } else { "off" };
                            print!("\r[mute changed to {s}]\nvizio> ");
                            let _ = std::io::stdout().flush();
                        }
                    } else if handle == H_EQ && v.len() == 1 {
                        if live_print.load(Ordering::SeqCst) {
                            let s = EqMode::from_u8(v[0])
                                .map(|m| m.as_str())
                                .unwrap_or("unknown");
                            print!("\r[EQ mode changed to {s}]\nvizio> ");
                            let _ = std::io::stdout().flush();
                        }
                    } else if live_print.load(Ordering::SeqCst) && handle != H_AUTH_STATUS {
                        print!("\r[notify 0x{handle:04x} -> {}]\nvizio> ", hex(&v));
                        let _ = std::io::stdout().flush();
                    }
                    let _ = tx.send((handle, v));
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

    fn drain(&self) -> Vec<(u16, Vec<u8>)> {
        self.rx.try_iter().collect()
    }

    fn wait_notify(&self, handle: u16, timeout: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok((h, v)) => {
                    if self.verbose {
                        eprintln!("  notify 0x{h:04x} -> {}", hex(&v));
                    }
                    if h == handle {
                        return Some(v);
                    }
                }
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                    return None;
                }
            }
        }
    }

    fn authenticate(&mut self) -> Result<()> {
        if let Err(e) = self.subscribe(H_AUTH_STATUS) {
            eprintln!("  (warning: {e})");
        }
        let _ = self.subscribe(H_VOLUME);
        let _ = self.subscribe(H_MUTE);
        let _ = self.subscribe(H_EQ);

        let tag = self.read(H_DEVICE_TAG).unwrap_or_default();
        self.drain();
        self.write_raw(H_AUTH_CHALLENGE, CLIENT_ID, Some(true))
            .context("sending client id")?;
        if self
            .wait_notify(H_AUTH_STATUS, Duration::from_millis(1500))
            .is_none()
        {
            std::thread::sleep(Duration::from_millis(500));
        }

        let challenge = self.read(H_AUTH_CHALLENGE).context("reading challenge")?;
        let mut resp = challenge.clone();
        resp.push(b':');
        resp.extend_from_slice(&AUTH_SUFFIX);
        self.write_raw(H_AUTH_RESPONSE, &resp, Some(true))
            .context("sending challenge response")?;
        match self.wait_notify(H_AUTH_STATUS, Duration::from_millis(1500)) {
            Some(v) if v.first() == Some(&1) => {}
            Some(v) => eprintln!("  warning: auth status notification was {}", hex(&v)),
            None => std::thread::sleep(Duration::from_millis(500)),
        }

        let token = self
            .read(H_AUTH_RESPONSE)
            .context("reading session token")?;
        if token.is_empty() {
            bail!("authentication failed (empty session token)");
        }
        if self.verbose {
            eprintln!(
                "  tag \"{}\", challenge \"{}\", session token \"{}\"",
                printable(&tag),
                printable(&challenge),
                printable(&token)
            );
        }
        self.token = Some(token);
        Ok(())
    }

    fn volume(&self) -> Result<u8> {
        let v = self
            .read(H_VOLUME)?
            .first()
            .copied()
            .ok_or_else(|| anyhow!("empty volume value"))?;
        *self.last_volume.lock().unwrap() = Some(v);
        Ok(v)
    }

    /// Write one volume value and return the level the soundbar reports back.
    /// Notifications repeating the old level (e.g. the unsolicited one sent
    /// right after subscribing) are skipped.
    fn write_volume(&self, cur: u8, level: u8) -> Result<u8> {
        self.drain();
        self.write_cmd(H_VOLUME, &[level])?;
        let deadline = Instant::now() + Duration::from_millis(1500);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.wait_notify(H_VOLUME, left) {
                Some(v) if v.first().is_some_and(|&x| x != cur) => return Ok(v[0]),
                Some(_) => continue,
                None => break,
            }
        }
        self.volume()
    }

    /// The soundbar only moves one step per write (writing 11 at volume 8
    /// yields 9), so walk to the target one level at a time, like the app
    /// does.
    fn set_volume(&self, level: u8) -> Result<u8> {
        let target = level.min(self.max_volume);
        if level > target {
            eprintln!("(clamped to max volume {target})");
        }
        let mut cur = self.volume()?;
        let mut stalls = 0;
        while cur != target {
            let next = if target > cur { cur + 1 } else { cur - 1 };
            let got = self.write_volume(cur, next)?;
            if got == cur {
                stalls += 1;
                if stalls >= 3 {
                    bail!("volume stuck at {cur} (target {target})");
                }
            } else {
                stalls = 0;
                if self.verbose {
                    eprintln!("  volume {cur} -> {got}");
                }
            }
            cur = got;
        }
        Ok(cur)
    }

    fn mute(&self) -> Result<bool> {
        let v = self.read(H_MUTE)?;
        Ok(v.first().copied().unwrap_or(0) != 0)
    }

    fn set_mute(&self, mute: bool) -> Result<bool> {
        self.drain();
        self.write_cmd(H_MUTE, &[if mute { 1 } else { 0 }])?;
        let _ = self.wait_notify(H_MUTE, Duration::from_millis(500));
        self.mute()
    }

    fn toggle_mute(&self) -> Result<bool> {
        let cur = self.mute()?;
        self.set_mute(!cur)
    }

    fn eq(&self) -> Result<EqMode> {
        let v = self.read(H_EQ)?;
        let byte = v
            .first()
            .copied()
            .ok_or_else(|| anyhow!("empty EQ value"))?;
        EqMode::from_u8(byte).ok_or_else(|| anyhow!("unknown EQ mode byte: {byte}"))
    }

    fn set_eq(&self, mode: EqMode) -> Result<EqMode> {
        self.drain();
        self.write_cmd(H_EQ, &[mode as u8])?;
        let _ = self.wait_notify(H_EQ, Duration::from_millis(500));
        self.eq()
    }

    fn snapshot_info(&self) -> HashMap<u16, Vec<u8>> {
        let mut map = HashMap::new();
        for &(h, _, _) in INFO_FIELDS {
            if let Ok(v) = self.read(h) {
                map.insert(h, v);
            }
        }
        map
    }

    fn snapshot_all(&self) -> HashMap<u16, Vec<u8>> {
        let mut map = HashMap::new();
        for c in &self.chars {
            if c.props & GattCharacteristicProperties::Read.0 != 0 {
                let h = self.value_handle(c);
                if let Ok(v) = self.read(h) {
                    map.insert(h, v);
                }
            }
        }
        map
    }

    fn subscribe_all_notify(&mut self) -> usize {
        let handles: Vec<u16> = self
            .chars
            .iter()
            .filter(|c| {
                c.props
                    & (GattCharacteristicProperties::Notify.0
                        | GattCharacteristicProperties::Indicate.0)
                    != 0
            })
            .map(|c| self.value_handle(c))
            .collect();
        let mut count = 0;
        for h in handles {
            if self.subscribe(h).is_ok() {
                count += 1;
            }
        }
        count
    }

    fn print_info(&self) -> Result<()> {
        println!(
            "Address      : {}",
            fmt_addr(self.device.BluetoothAddress()?)
        );
        println!("Windows name : {}", self.device.Name()?);
        println!(
            "Connected    : {}",
            self.device.ConnectionStatus()? == BluetoothConnectionStatus::Connected
        );
        if let Ok(p) = self.device.DeviceInformation().and_then(|d| d.Pairing()) {
            println!("OS paired    : {}", p.IsPaired()?);
        }
        if let Some(t) = &self.token {
            println!("Session token: {}", printable(t));
        }
        // Standard GATT strings (Device Information service etc.), if exposed.
        for c in &self.chars {
            let label = match short_uuid(&c.uuid) {
                Some(0x2A24) => "Model",
                Some(0x2A25) => "Serial",
                Some(0x2A26) => "Firmware",
                Some(0x2A27) => "Hardware",
                Some(0x2A28) => "Software",
                Some(0x2A29) => "Manufacturer",
                _ => continue,
            };
            if let Ok(v) = self.read(self.value_handle(c)) {
                println!(
                    "{label:<13}: {}",
                    String::from_utf8_lossy(&v).trim_end_matches('\0')
                );
            }
        }
        for &(h, label, fmt) in INFO_FIELDS {
            if self.char_for(h).is_err() {
                continue;
            }
            match self.read(h) {
                Ok(v) => match fmt {
                    Fmt::Str => println!("{label:<13}: {}", String::from_utf8_lossy(&v)),
                    Fmt::U8 if v.len() == 1 => println!("{label:<13}: {}", v[0]),
                    Fmt::U8 => println!("{label:<13}: {}", hex(&v)),
                    Fmt::Bool => {
                        let b = v.first().copied().unwrap_or(0) != 0;
                        println!("{label:<13}: {}", if b { "on (muted)" } else { "off" });
                    }
                    Fmt::Eq => {
                        if let Some(&byte) = v.first() {
                            if let Some(mode) = EqMode::from_u8(byte) {
                                println!("{label:<13}: {mode} ({byte})");
                            } else {
                                println!("{label:<13}: unknown ({byte})");
                            }
                        }
                    }
                },
                Err(e) => println!("{label:<13}: <{e}>"),
            }
        }
        Ok(())
    }

    fn dump(&self) -> Result<()> {
        println!(
            "handle offset: {:+} (Windows AttributeHandle -> ATT value handle)",
            self.offset
        );
        let mut last_svc = None;
        let mut chars: Vec<&CharInfo> = self.chars.iter().collect();
        chars.sort_by_key(|c| c.attr_handle);
        for c in chars {
            if last_svc != Some(c.service_uuid) {
                println!("\nService {}", uuid_str(&c.service_uuid));
                last_svc = Some(c.service_uuid);
            }
            let h = self.value_handle(c);
            let mut line = format!(
                "  0x{h:04x}  {:<38} [{}]",
                uuid_str(&c.uuid),
                props_str(c.props)
            );
            if c.props & GattCharacteristicProperties::Read.0 != 0 {
                match self.read(h) {
                    Ok(v) => line += &format!("  = {}  \"{}\"", hex(&v), printable(&v)),
                    Err(e) => line += &format!("  <{e}>"),
                }
            }
            println!("{line}");
        }
        Ok(())
    }

    fn close(self) {
        for s in &self.services {
            let _ = s.Close();
        }
        if let Some(s) = &self.session {
            let _ = s.Close();
        }
        let _ = self.device.Close();
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_scan(cli: &Cli) -> Result<()> {
    println!("Paired devices:");
    for (name, addr, kind) in paired_devices().unwrap_or_default() {
        println!("  {}  {:<8} {name}", fmt_addr(addr), kind);
    }
    println!("\nScanning BLE for {}s...", cli.scan_secs);
    let mut seen: Vec<_> = scan(cli.scan_secs, None)?.into_iter().collect();
    seen.sort_by_key(|(_, (_, rssi))| -rssi);
    for (addr, (name, rssi)) in seen {
        let mark = if name.to_lowercase().contains(&cli.name.to_lowercase()) {
            " <=="
        } else {
            ""
        };
        println!("  {}  {rssi:>4} dBm  {name}{mark}", fmt_addr(addr));
    }
    Ok(())
}

fn cmd_pair(cli: &Cli) -> Result<()> {
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

fn cmd_unpair(cli: &Cli) -> Result<()> {
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

fn apply_volume(sb: &Soundbar, action: &VolAction) -> Result<()> {
    match action {
        VolAction::Get => println!("Volume: {}", sb.volume()?),
        VolAction::Set { level } => println!("Volume: {}", sb.set_volume(*level)?),
        VolAction::Up { steps } => {
            let cur = sb.volume()?;
            println!(
                "Volume: {cur} -> {}",
                sb.set_volume(cur.saturating_add(*steps))?
            );
        }
        VolAction::Down { steps } => {
            let cur = sb.volume()?;
            println!(
                "Volume: {cur} -> {}",
                sb.set_volume(cur.saturating_sub(*steps))?
            );
        }
    }
    Ok(())
}

fn apply_mute(sb: &Soundbar, action: &MuteAction) -> Result<()> {
    match action {
        MuteAction::Get => {
            let m = sb.mute()?;
            println!("Mute: {}", if m { "on" } else { "off" });
        }
        MuteAction::On => {
            sb.set_mute(true)?;
            println!("Mute: on");
        }
        MuteAction::Off => {
            sb.set_mute(false)?;
            println!("Mute: off");
        }
        MuteAction::Toggle => {
            let m = sb.toggle_mute()?;
            println!("Mute: {}", if m { "on" } else { "off" });
        }
    }
    Ok(())
}

fn apply_eq(sb: &Soundbar, mode: Option<EqMode>) -> Result<()> {
    match mode {
        Some(m) => {
            let res = sb.set_eq(m)?;
            println!("EQ Mode: {res} ({})", res as u8);
        }
        None => {
            let res = sb.eq()?;
            println!("EQ Mode: {res} ({})", res as u8);
        }
    }
    Ok(())
}

fn cmd_monitor(sb: &mut Soundbar) -> Result<()> {
    let handles: Vec<u16> = sb
        .chars
        .iter()
        .filter(|c| {
            c.props
                & (GattCharacteristicProperties::Notify.0
                    | GattCharacteristicProperties::Indicate.0)
                != 0
        })
        .map(|c| sb.value_handle(c))
        .collect();
    for h in handles {
        match sb.subscribe(h) {
            Ok(()) => println!("subscribed 0x{h:04x}"),
            Err(e) => println!("{e}"),
        }
    }
    println!("Listening for notifications (Ctrl+C to quit)...");
    loop {
        if let Ok((h, v)) = sb.rx.recv() {
            println!("0x{h:04x} -> {}  \"{}\"", hex(&v), printable(&v));
        }
    }
}

fn preprocess_shell_line(line: &str) -> Option<String> {
    let line = match line.split_once('#') {
        Some((before, _)) => before,
        None => line,
    }
    .trim();

    if line.is_empty() {
        return None;
    }

    // Handle bare integer volume levels (e.g. "45" -> "set 45").
    if line.parse::<u8>().is_ok() {
        return Some(format!("set {line}"));
    }

    // Handle "-" shortcuts (e.g. "-" -> "down", "- 3" -> "down 3", "-3" ->
    // "down 3"). Avoid intercepting flags like "-h" or "--help".
    if let Some(rest) = line.strip_prefix('-') {
        let rest = rest.trim();
        if rest.is_empty() {
            return Some("down".to_string());
        }
        if !rest.starts_with('-')
            && rest != "h"
            && (rest.chars().all(|c| c.is_ascii_digit())
                || rest.starts_with(|c: char| c.is_ascii_digit() || c.is_whitespace()))
        {
            return Some(format!("down {rest}"));
        }
    }

    // Handle "+" shortcuts (e.g. "+" -> "up", "+ 3" -> "up 3", "+3" -> "up 3").
    if let Some(rest) = line.strip_prefix('+') {
        let rest = rest.trim();
        if rest.is_empty() {
            return Some("up".to_string());
        }
        return Some(format!("up {rest}"));
    }

    Some(line.to_string())
}

fn diff_info(sb: &Soundbar, snap_info: &mut Option<HashMap<u16, Vec<u8>>>) {
    let cur = sb.snapshot_info();
    if let Some(prev) = snap_info.take() {
        let mut diffs = Vec::new();
        for &(h, label, _) in INFO_FIELDS {
            let old = prev.get(&h);
            let new = cur.get(&h);
            if old != new {
                diffs.push((h, label, old, new));
            }
        }
        if diffs.is_empty() {
            println!("No changes detected across {} fields.", cur.len());
        } else {
            println!("Changes detected:");
            for (h, label, old, new) in diffs {
                let fmt_val = |v: Option<&Vec<u8>>| match v {
                    Some(b) if h == H_EQ && b.len() == 1 => {
                        let name = EqMode::from_u8(b[0])
                            .map(|m| m.as_str())
                            .unwrap_or("unknown");
                        format!("{name} ({})", b[0])
                    }
                    Some(b) if b.len() == 1 => format!("{} (0x{:02x})", b[0], b[0]),
                    Some(b) => hex(b),
                    None => "<none>".to_string(),
                };
                println!(
                    "  0x{h:04x} ({label}): {} -> {}",
                    fmt_val(old),
                    fmt_val(new)
                );
            }
        }
    } else {
        println!(
            "Baseline snapshot saved ({} fields). Press a button on the remote, then type 'diff'.",
            cur.len()
        );
    }
    *snap_info = Some(cur);
}

fn diff_all(sb: &Soundbar, snap_all: &mut Option<HashMap<u16, Vec<u8>>>) {
    let cur = sb.snapshot_all();
    if let Some(prev) = snap_all.take() {
        let mut diffs = Vec::new();
        for (&h, new) in &cur {
            if let Some(old) = prev.get(&h) {
                if old != new {
                    diffs.push((h, old, new));
                }
            }
        }
        diffs.sort_by_key(|&(h, _, _)| h);
        if diffs.is_empty() {
            println!("No changes detected across {} handles.", cur.len());
        } else {
            println!("Changes detected across all handles:");
            for (h, old, new) in diffs {
                let label = INFO_FIELDS
                    .iter()
                    .find(|(ih, _, _)| *ih == h)
                    .map(|(_, l, _)| *l)
                    .unwrap_or("unknown");
                let fmt_val = |b: &[u8]| {
                    if h == H_EQ && b.len() == 1 {
                        let name = EqMode::from_u8(b[0])
                            .map(|m| m.as_str())
                            .unwrap_or("unknown");
                        format!("{name} ({})", b[0])
                    } else if b.len() == 1 {
                        format!("{} (0x{:02x})", b[0], b[0])
                    } else {
                        format!("{} \"{}\"", hex(b), printable(b))
                    }
                };
                println!(
                    "  0x{h:04x} ({label}): {} -> {}",
                    fmt_val(old),
                    fmt_val(new)
                );
            }
        }
    } else {
        println!(
            "Baseline snapshot of ALL handles saved ({} handles). Press a button on the remote, then type 'diff all'.",
            cur.len()
        );
    }
    *snap_all = Some(cur);
}

fn execute_device_cmd(sb: &mut Soundbar, cmd: &DeviceCmd) -> Result<()> {
    match cmd {
        DeviceCmd::Info => sb.print_info(),
        DeviceCmd::Dump => sb.dump(),
        DeviceCmd::Volume { action } => {
            apply_volume(sb, action.as_ref().unwrap_or(&VolAction::Get))
        }
        DeviceCmd::Mute { action } => {
            apply_mute(sb, action.as_ref().unwrap_or(&MuteAction::Toggle))
        }
        DeviceCmd::Eq { mode } => apply_eq(sb, *mode),
        DeviceCmd::Read { handle } => {
            let v = sb.read(parse_handle(handle)?)?;
            println!("{}  \"{}\"", hex(&v), printable(&v));
            Ok(())
        }
        DeviceCmd::Write {
            handle,
            hex: h,
            no_token,
            with_response,
        } => {
            let handle = parse_handle(handle)?;
            let data = parse_hex(&h.join(""))?;
            if *no_token {
                sb.write_raw(handle, &data, Some(*with_response))?;
            } else {
                sb.write_cmd(handle, &data)?;
            }
            if let Some(v) = sb.wait_notify(handle, Duration::from_millis(1000)) {
                println!("notify 0x{handle:04x} -> {}", hex(&v));
            }
            Ok(())
        }
        DeviceCmd::Monitor => cmd_monitor(sb),
    }
}

fn cmd_shell(sb: &mut Soundbar) -> Result<()> {
    println!(
        "Commands: + / up [n], - / down [n], <number> / set <n>, get, m / mute [on|off], \
         eq [movie|music|game|direct], d / diff [all], snap, watch, info, dump, read <h>, write <h> <hex>, q (or 'help')"
    );
    let mut snap_info: Option<HashMap<u16, Vec<u8>>> = None;
    let mut snap_all: Option<HashMap<u16, Vec<u8>>> = None;
    let stdin = std::io::stdin();
    loop {
        // Queued notifications were either already printed live or belong to
        // commands that reported their own result.
        sb.drain();
        print!("vizio> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        sb.live_print.store(true, Ordering::SeqCst);
        let n_read = stdin.lock().read_line(&mut line);
        sb.live_print.store(false, Ordering::SeqCst);
        if n_read? == 0 {
            break;
        }

        let Some(normalized) = preprocess_shell_line(&line) else {
            continue;
        };

        let args = normalized.split_whitespace();
        let cmd = match ShellCmd::try_parse_from(args) {
            Ok(cmd) => cmd,
            Err(e) => {
                let _ = e.print();
                continue;
            }
        };

        let res = match cmd {
            ShellCmd::Quit => break,
            ShellCmd::Device(op) => execute_device_cmd(sb, &op),
            ShellCmd::Up { steps } => apply_volume(sb, &VolAction::Up { steps }),
            ShellCmd::Down { steps } => apply_volume(sb, &VolAction::Down { steps }),
            ShellCmd::Set { level } => apply_volume(sb, &VolAction::Set { level }),
            ShellCmd::Get => apply_volume(sb, &VolAction::Get),
            ShellCmd::Watch => {
                let count = sb.subscribe_all_notify();
                println!(
                    "Subscribed to {count} characteristics. Notifications will print live at prompt."
                );
                Ok(())
            }
            ShellCmd::Snap => {
                snap_info = Some(sb.snapshot_info());
                snap_all = Some(sb.snapshot_all());
                println!("Snapshots updated as new baseline.");
                Ok(())
            }
            ShellCmd::Diff { all, target } => {
                let is_all = all || target.as_deref() == Some("all");
                if is_all {
                    diff_all(sb, &mut snap_all);
                } else {
                    diff_info(sb, &mut snap_info);
                }
                Ok(())
            }
        };

        if let Err(e) = res {
            println!("error: {e:#}");
        }
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::Scan => return cmd_scan(&cli),
        Cmd::Pair => return cmd_pair(&cli),
        Cmd::Unpair => return cmd_unpair(&cli),
        _ => {}
    }

    let mut sb = Soundbar::connect(&cli)?;
    let result = (|| -> Result<()> {
        if matches!(cli.cmd, Cmd::Op(DeviceCmd::Dump)) {
            // Dump before auth so it works even if the handshake fails.
            return sb.dump();
        }
        sb.authenticate()
            .context("authentication handshake failed")?;
        match &cli.cmd {
            Cmd::Shell => cmd_shell(&mut sb),
            Cmd::Op(op) => execute_device_cmd(&mut sb, op),
            Cmd::Scan | Cmd::Pair | Cmd::Unpair => unreachable!(),
        }
    })();
    sb.close();
    result
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        if format!("{e:#}").contains("0x800710DF") {
            eprintln!(
                "hint: Bluetooth looks turned off - enable it in Windows Settings > Bluetooth & devices."
            );
        }
        std::process::exit(1);
    }
}
