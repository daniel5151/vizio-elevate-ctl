//! vizio-ctl: control a VIZIO Elevate soundbar over Bluetooth LE

mod bt;

use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::channel;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

const H_MODEL: u16 = 0x004a;
const H_SERIAL: u16 = 0x004c;
const H_FIRMWARE: u16 = 0x004e;
const H_AUDIO_FORMAT: u16 = 0x0050;
/// Read-only system state (0x0052). Value is 0x01 when active; writes rejected
/// by hardware (0x80650003).
const H_STATE_52: u16 = 0x0052;
const H_DEVICE_NAME: u16 = 0x0054;
const H_DEVICE_TAG: u16 = 0x0056;
const H_POWER_LIGHT: u16 = 0x0064;
const H_MUTE: u16 = 0x006a;
const H_AV_DELAY: u16 = 0x006d;
const H_FACTORY_RESET: u16 = 0x0070;
const H_EQ: u16 = 0x0076;
const H_VOLUME: u16 = 0x0079;
const H_INPUT: u16 = 0x007c;
const H_SPEAKER_COLOR: u16 = 0x007f;
const H_STATE_82: u16 = 0x0082;
const H_AUTO_HDMI: u16 = 0x0085;
const H_LEVEL_BASS: u16 = 0x0089;
const H_LEVEL_TREBLE: u16 = 0x008c;
const H_DIALOG: u16 = 0x008f;
const H_LEVEL_CENTER: u16 = 0x0092;
const H_LEVEL_SUBWOOFER: u16 = 0x0095;
const H_LEVEL_SURROUND: u16 = 0x0098;
const H_LEVEL_BALANCE: u16 = 0x009b;
const H_VIRTUAL_X: u16 = 0x00a2;
const H_TRUVOLUME: u16 = 0x00a5;
const H_NIGHT_MODE: u16 = 0x00a8;
const H_SURROUND: u16 = 0x00ab;
const H_ELEVATION_MODE: u16 = 0x00ae;
const H_AUTH_STATUS: u16 = 0x00bf;
const H_AUTH_CHALLENGE: u16 = 0x00c2;
const H_AUTH_RESPONSE: u16 = 0x00c5;

/// Handles used to auto-detect how Windows' `AttributeHandle` maps to ATT value
/// handles.
pub const KNOWN_HANDLES: &[u16] = &[
    H_MODEL,
    H_SERIAL,
    H_FIRMWARE,
    H_AUDIO_FORMAT,
    H_STATE_52,
    H_DEVICE_NAME,
    H_DEVICE_TAG,
    H_POWER_LIGHT,
    H_MUTE,
    H_AV_DELAY,
    H_FACTORY_RESET,
    H_EQ,
    H_VOLUME,
    H_INPUT,
    H_SPEAKER_COLOR,
    H_STATE_82,
    H_AUTO_HDMI,
    H_LEVEL_BASS,
    H_LEVEL_TREBLE,
    H_DIALOG,
    H_LEVEL_CENTER,
    H_LEVEL_SUBWOOFER,
    H_LEVEL_SURROUND,
    H_LEVEL_BALANCE,
    H_VIRTUAL_X,
    H_TRUVOLUME,
    H_NIGHT_MODE,
    H_SURROUND,
    H_ELEVATION_MODE,
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
    (H_MODEL, "Model", Fmt::Str),
    (H_SERIAL, "Serial", Fmt::Str),
    (H_FIRMWARE, "Firmware", Fmt::Str),
    (H_AUDIO_FORMAT, "Audio format", Fmt::Str),
    (H_DEVICE_NAME, "Device name", Fmt::Str),
    (H_DEVICE_TAG, "Device tag", Fmt::Str),
    (H_VOLUME, "Volume", Fmt::U8),
    (H_MUTE, "Mute", Fmt::Bool),
    (H_EQ, "EQ Mode", Fmt::Eq),
    (H_SURROUND, "Surround Mode", Fmt::Surround),
    (H_VIRTUAL_X, "Virtual:X", Fmt::Bool),
    (H_TRUVOLUME, "TruVolume", Fmt::Bool),
    (H_NIGHT_MODE, "Night Mode", Fmt::Bool),
    (H_ELEVATION_MODE, "Elevation Mode", Fmt::Elevation),
    (H_SPEAKER_COLOR, "Speaker Color", Fmt::SpeakerColor),
    (H_POWER_LIGHT, "Power Light", Fmt::Bool),
    (H_AUTO_HDMI, "Auto HDMI", Fmt::Bool),
    (H_AV_DELAY, "AV Delay", Fmt::U8),
    (H_DIALOG, "Clear Dialog", Fmt::U8),
    (H_LEVEL_BASS, "Bass", Fmt::I8),
    (H_LEVEL_TREBLE, "Treble", Fmt::I8),
    (H_LEVEL_CENTER, "Center", Fmt::I8),
    (H_LEVEL_SUBWOOFER, "Subwoofer", Fmt::I8),
    (H_LEVEL_SURROUND, "Surround Level", Fmt::I8),
    (H_LEVEL_BALANCE, "Surround Balance", Fmt::I8),
    (H_INPUT, "Input Source", Fmt::Input),
    (H_STATE_82, "0x0082 (unknown)", Fmt::U8),
    (H_STATE_52, "0x0052 (unknown, read-only)", Fmt::U8),
];

/// Characteristics subscribed to for notifications upon authentication.
const DEFAULT_SUBSCRIBE_HANDLES: &[u16] = &[
    H_INPUT,
    H_VOLUME,
    H_MUTE,
    H_EQ,
    H_SURROUND,
    H_ELEVATION_MODE,
    H_SPEAKER_COLOR,
    H_POWER_LIGHT,
    H_AUTO_HDMI,
    H_AV_DELAY,
    H_LEVEL_BASS,
    H_LEVEL_TREBLE,
    H_LEVEL_CENTER,
    H_LEVEL_SUBWOOFER,
    H_LEVEL_SURROUND,
    H_LEVEL_BALANCE,
    H_DIALOG,
    H_VIRTUAL_X,
    H_TRUVOLUME,
    H_NIGHT_MODE,
];

#[derive(Clone, Copy)]
enum Fmt {
    Str,
    U8,
    I8,
    Bool,
    Input,
    Eq,
    Surround,
    Elevation,
    SpeakerColor,
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Clone)]
#[command(version, about = "Control a VIZIO Elevate soundbar over Bluetooth LE")]
pub(crate) struct Cli {
    /// BLE address of the soundbar (AA:BB:CC:DD:EE:FF). Remembered after first
    /// success.
    #[arg(short, long, global = true)]
    pub(crate) address: Option<String>,

    /// Substring of the advertised/paired device name to look for.
    #[arg(short, long, global = true, default_value = "VIZIO")]
    pub(crate) name: String,

    /// How long to scan for the soundbar when it isn't already known.
    #[arg(long, global = true, default_value_t = 10)]
    pub(crate) scan_secs: u64,

    /// Print every GATT operation.
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,

    /// Never raise the volume above this level.
    #[arg(long, global = true, default_value_t = MAX_VOLUME)]
    pub(crate) max_volume: u8,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Clone)]
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
    /// Get or set the audio input mode (bluetooth, hdmi-earc).
    #[command(alias = "in", alias = "source")]
    Input {
        /// Mode to set (bluetooth, hdmi-earc). Omit to print current input.
        mode: Option<InputMode>,
    },
    /// Get or set the EQ mode.
    Eq {
        /// Mode to set (movie, music, game, direct). Omit to print current
        /// mode.
        mode: Option<EqMode>,
    },
    /// Get or set the surround mode.
    #[command(alias = "surr")]
    Surround {
        /// Mode to set (off, on, dual, front). Omit to print current mode.
        mode: Option<SurroundMode>,
    },
    /// Get or set the elevate speakers position (up, wide, auto).
    #[command(alias = "elev", alias = "elevation")]
    Position {
        /// Mode to set (up, wide, auto). Omit to print current position.
        mode: Option<ElevationMode>,
    },
    /// Get or set the elevate speakers LED color.
    #[command(alias = "speaker-color", alias = "led-color")]
    Color {
        /// Color to set (off, rainbow, light-blue, yellow, purple, white, cyan,
        /// magenta). Omit to print current color.
        color: Option<SpeakerColor>,
    },
    /// Get, toggle, or set the power indicator light (LED).
    #[command(alias = "power-led", alias = "light")]
    PowerLight {
        #[command(subcommand)]
        action: Option<OnOffAction>,
    },
    /// Get, toggle, or set auto HDMI input switching.
    #[command(alias = "hdmi")]
    AutoHdmi {
        #[command(subcommand)]
        action: Option<OnOffAction>,
    },
    /// Get or set AV delay (lip sync delay 0-8).
    #[command(alias = "delay")]
    AvDelay {
        /// Delay value to set (0-8). Omit to print current delay.
        delay: Option<u8>,
    },
    /// Get or set clear dialog level (0-10).
    #[command(alias = "dialogue", alias = "clear-dialog")]
    Dialog {
        /// Dialog level to set (0-10). Omit to print current level.
        level: Option<u8>,
    },
    /// Get, toggle, or set DTS Virtual:X sound effect.
    #[command(alias = "vx", alias = "virtualx", alias = "dts-x")]
    VirtualX {
        #[command(subcommand)]
        action: Option<OnOffAction>,
    },
    /// Get, toggle, or set DTS TruVolume volume leveling.
    #[command(name = "truvolume", alias = "truvol", alias = "tru-volume")]
    TruVolume {
        #[command(subcommand)]
        action: Option<OnOffAction>,
    },
    /// Get, toggle, or set night mode.
    #[command(alias = "night")]
    NightMode {
        #[command(subcommand)]
        action: Option<OnOffAction>,
    },
    /// Get or set audio levels (bass, treble, center, subwoofer, surround,
    /// balance).
    #[command(alias = "level")]
    Levels {
        #[command(subcommand)]
        target: Option<LevelCmd>,
    },
    /// Get or set bass level (-10 to +10).
    Bass {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set treble level (-10 to +10).
    Treble {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set center channel level (-10 to +10).
    Center {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set subwoofer level (-10 to +10).
    #[command(alias = "sub")]
    Subwoofer {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set surround level (-10 to +10).
    #[command(alias = "surr-level")]
    SurroundLevel {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set surround balance (-10 to +10).
    #[command(alias = "balance")]
    SurroundBalance {
        #[arg(allow_negative_numbers = true)]
        balance: Option<i8>,
    },
    /// Factory reset the soundbar (restores all settings to factory defaults).
    #[command(alias = "reset")]
    FactoryReset {
        /// Confirm factory reset without an interactive prompt.
        #[arg(long)]
        yes_really: bool,
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
    /// Disconnect and release the Bluetooth connection (allowing phone app to
    /// connect).
    #[command(alias = "disc", alias = "release")]
    Disconnect,
    /// Reconnect and re-authenticate with the soundbar.
    #[command(alias = "rec", alias = "connect")]
    Reconnect,
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

/// Trait for soundbar configuration enums that map to/from a single byte.
pub trait VizioEnum: Sized + Copy + 'static {
    /// Decode a byte into the enum variant.
    fn from_u8(v: u8) -> Option<Self>;
    /// Human-readable display string.
    fn as_str(&self) -> &'static str;
    /// Encode the enum variant to a byte.
    fn to_u8(&self) -> u8;
}

macro_rules! vizio_enum {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$var_meta:meta])*
                $var:ident = $val:expr, $display:literal, [ $($alias:literal),* $(,)? ]
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        $vis enum $name {
            $(
                $(#[$var_meta])*
                $var = $val,
            )+
        }

        impl VizioEnum for $name {
            fn from_u8(v: u8) -> Option<Self> {
                match v {
                    $( $val => Some(Self::$var), )+
                    _ => None,
                }
            }

            fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$var => $display, )+
                }
            }

            fn to_u8(&self) -> u8 {
                *self as u8
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", <Self as VizioEnum>::as_str(self))
            }
        }

        impl std::str::FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(s: &str) -> Result<Self> {
                let norm = s.to_ascii_lowercase().replace(['-', '_', ' '], "");
                match norm.as_str() {
                    $(
                        stringify!($val) $( | $alias )* => Ok(Self::$var),
                    )+
                    _ => bail!(
                        concat!("unknown ", stringify!($name), " {:?} (expected ",
                            vizio_enum!(@join_names $($display),+), ")"
                        ),
                        s
                    ),
                }
            }
        }
    };
    (@join_names $first:literal) => { $first };
    (@join_names $first:literal, $($rest:literal),+) => {
        concat!($first, ", ", vizio_enum!(@join_names $($rest),+))
    };
}

vizio_enum! {
    /// Audio input mode.
    #[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum InputMode {
        /// Bluetooth audio input.
        #[value(alias = "bt")]
        Bluetooth = 1, "Bluetooth", ["bluetooth", "bt"],
        /// HDMI-eARC audio input.
        #[value(alias = "earc", alias = "hdmi")]
        HdmiEarc = 2, "HDMI-eARC", ["hdmiearc", "hdmi", "earc"],
    }
}

vizio_enum! {
    /// EQ mode preset.
    #[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum EqMode {
        /// Movie EQ preset.
        Movie = 0, "Movie", ["movie"],
        /// Music EQ preset.
        Music = 1, "Music", ["music"],
        /// Game EQ preset.
        Game = 2, "Game", ["game"],
        /// Direct EQ preset (unprocessed source audio).
        Direct = 3, "Direct", ["direct"],
    }
}

vizio_enum! {
    /// Surround mode preset.
    #[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SurroundMode {
        /// Surround sound off.
        Off = 0, "Off", ["off"],
        /// Surround sound on.
        On = 1, "On", ["on"],
        /// Dual surround mode.
        Dual = 2, "Dual", ["dual"],
        /// Front surround mode.
        #[value(alias = "frnt")]
        Front = 3, "Front", ["front", "frnt"],
    }
}

vizio_enum! {
    /// Elevation speakers position.
    #[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ElevationMode {
        /// Elevation speakers facing up.
        Up = 0, "Up", ["up"],
        /// Elevation speakers facing wide.
        Wide = 1, "Wide", ["wide"],
        /// Automatic elevation speakers position.
        Auto = 2, "Auto", ["auto"],
    }
}

vizio_enum! {
    /// Elevate speakers LED color.
    #[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SpeakerColor {
        /// LED off.
        Off = 0, "Off", ["off"],
        /// Rainbow animation effect.
        #[value(alias = "rainbow-effect", alias = "rainbow_effect")]
        Rainbow = 1, "Rainbow", ["rainbow", "rainboweffect"],
        /// Light blue.
        #[value(alias = "lightblue", alias = "light_blue")]
        LightBlue = 2, "Light Blue", ["lightblue"],
        /// Yellow.
        Yellow = 3, "Yellow", ["yellow"],
        /// Purple.
        Purple = 4, "Purple", ["purple"],
        /// White.
        White = 5, "White", ["white"],
        /// Cyan.
        Cyan = 6, "Cyan", ["cyan"],
        /// Magenta.
        Magenta = 7, "Magenta", ["magenta"],
    }
}

fn field_by_handle(handle: u16) -> Option<(&'static str, Fmt)> {
    INFO_FIELDS
        .iter()
        .find(|(h, _, _)| *h == handle)
        .map(|(_, label, fmt)| (*label, *fmt))
}

fn format_enum_val<T: VizioEnum + std::fmt::Display>(v: &[u8], detailed: bool) -> String {
    match v.first() {
        Some(&byte) => match T::from_u8(byte) {
            Some(mode) => {
                if detailed {
                    format!("{mode} ({byte})")
                } else {
                    format!("{mode}")
                }
            }
            None => format!("unknown ({byte})"),
        },
        None => hex(v),
    }
}

fn format_setting_value(fmt: Fmt, handle: u16, v: &[u8], detailed: bool) -> String {
    match fmt {
        Fmt::Str => String::from_utf8_lossy(v)
            .trim_end_matches('\0')
            .trim()
            .to_string(),
        Fmt::U8 if v.len() == 1 => format!("{}", v[0]),
        Fmt::U8 => hex(v),
        Fmt::I8 if v.len() == 1 => fmt_level(v[0] as i8),
        Fmt::I8 => hex(v),
        Fmt::Bool => {
            let b = v.first().copied().unwrap_or(0) != 0;
            if handle == H_MUTE {
                if b { "on (muted)" } else { "off" }.to_string()
            } else {
                if b { "on" } else { "off" }.to_string()
            }
        }
        Fmt::Input => format_enum_val::<InputMode>(v, detailed),
        Fmt::Eq => format_enum_val::<EqMode>(v, detailed),
        Fmt::Surround => format_enum_val::<SurroundMode>(v, detailed),
        Fmt::Elevation => format_enum_val::<ElevationMode>(v, detailed),
        Fmt::SpeakerColor => format_enum_val::<SpeakerColor>(v, detailed),
    }
}

#[derive(Subcommand, Clone)]
enum OnOffAction {
    /// Print current state.
    #[command(alias = "status")]
    Get,
    /// Turn on.
    #[command(alias = "1")]
    On,
    /// Turn off.
    #[command(alias = "0")]
    Off,
    /// Toggle state.
    #[command(alias = "t")]
    Toggle,
}

#[derive(Subcommand, Clone)]
enum LevelCmd {
    /// Get or set bass level (-10 to +10).
    Bass {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set treble level (-10 to +10).
    Treble {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set center level (-10 to +10).
    Center {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set subwoofer level (-10 to +10).
    #[command(alias = "sub")]
    Subwoofer {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set surround level (-10 to +10).
    #[command(alias = "surr")]
    Surround {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
    /// Get or set surround balance (-10 to +10).
    Balance {
        #[arg(allow_negative_numbers = true)]
        level: Option<i8>,
    },
}

pub(crate) fn fmt_level(val: i8) -> String {
    if val > 0 {
        format!("+{val}")
    } else {
        format!("{val}")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn printable(b: &[u8]) -> String {
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

// ---------------------------------------------------------------------------
// Soundbar connection
// ---------------------------------------------------------------------------

struct Soundbar {
    cli: Cli,
    conn: Option<bt::BleConnection>,
    on_notify: bt::NotifyHandler,
    rx: Receiver<(u16, Vec<u8>)>,
    subscribed: Vec<u16>,
    token: Option<Vec<u8>>,
    verbose: bool,
    max_volume: u8,
    last_volume: Arc<Mutex<Option<u8>>>,
    live_print: Arc<AtomicBool>,
}

impl Soundbar {
    fn connect(cli: &Cli) -> Result<Self> {
        let (tx, rx) = channel();
        let last_volume: Arc<Mutex<Option<u8>>> = Arc::default();
        let live_print: Arc<AtomicBool> = Arc::default();

        let tx_clone = tx.clone();
        let last_vol = last_volume.clone();
        let live_pr = live_print.clone();

        let on_notify: bt::NotifyHandler = Arc::new(move |handle: u16, v: Vec<u8>| {
            if handle == H_VOLUME && v.len() == 1 {
                let mut last = last_vol.lock().unwrap();
                let changed = *last != Some(v[0]);
                *last = Some(v[0]);
                if changed && live_pr.load(Ordering::SeqCst) {
                    print!("\r[volume changed to {}]\nvizio> ", v[0]);
                    let _ = std::io::stdout().flush();
                }
            } else if live_pr.load(Ordering::SeqCst) && handle != H_AUTH_STATUS {
                if let Some((label, fmt)) = field_by_handle(handle) {
                    let val = format_setting_value(fmt, handle, &v, false);
                    print!("\r[{label} changed to {val}]\nvizio> ");
                } else {
                    print!("\r[notify 0x{handle:04x} -> {}]\nvizio> ", hex(&v));
                }
                let _ = std::io::stdout().flush();
            }
            let _ = tx_clone.send((handle, v));
        });

        let conn = bt::BleConnection::connect(cli, on_notify.clone())?;
        Ok(Self {
            cli: cli.clone(),
            conn: Some(conn),
            on_notify,
            rx,
            subscribed: Vec::new(),
            token: None,
            verbose: cli.verbose,
            max_volume: cli.max_volume.min(MAX_VOLUME),
            last_volume,
            live_print,
        })
    }

    /// Returns true if the soundbar is currently connected.
    fn is_connected(&self) -> bool {
        self.conn.as_ref().is_some_and(|c| c.is_connected())
    }

    /// Explicitly disconnect from the soundbar, releasing the connection
    /// so the mobile app or another device can connect.
    fn disconnect(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.close();
            println!("Disconnected from soundbar.");
        } else {
            println!("Already disconnected.");
        }
        self.token = None;
    }

    /// Reconnect and re-authenticate with the soundbar.
    fn reconnect(&mut self) -> Result<()> {
        if let Some(conn) = self.conn.take() {
            conn.close();
        }
        self.token = None;
        let prev_subscribed = std::mem::take(&mut self.subscribed);

        eprintln!("Connecting to soundbar...");
        let conn = bt::BleConnection::connect(&self.cli, self.on_notify.clone())?;
        self.conn = Some(conn);
        eprintln!("Authenticating...");
        self.authenticate()
            .context("authentication handshake failed")?;

        for h in prev_subscribed {
            let _ = self.subscribe(h);
        }

        eprintln!("Connected and authenticated.");
        Ok(())
    }

    /// Ensure we are connected and authenticated before running an operation.
    fn ensure_connected(&mut self) -> Result<()> {
        if !self.is_connected() {
            self.reconnect()?;
        }
        Ok(())
    }

    fn read(&mut self, handle: u16) -> Result<Vec<u8>> {
        self.ensure_connected()?;
        let conn = self.conn.as_ref().unwrap();
        match conn.read(handle) {
            Ok(v) => Ok(v),
            Err(e) => {
                if !self.is_connected() {
                    eprintln!("  connection lost, reconnecting...");
                    self.reconnect()?;
                    self.conn.as_ref().unwrap().read(handle)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn write_raw(&mut self, handle: u16, data: &[u8], with_response: Option<bool>) -> Result<()> {
        self.ensure_connected()?;
        let conn = self.conn.as_ref().unwrap();
        match conn.write(handle, data, with_response) {
            Ok(()) => Ok(()),
            Err(e) => {
                if !self.is_connected() {
                    eprintln!("  connection lost, reconnecting...");
                    self.reconnect()?;
                    self.conn
                        .as_ref()
                        .unwrap()
                        .write(handle, data, with_response)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Write `"<token>:" + payload` like the app does for every control.
    fn write_cmd(&mut self, handle: u16, payload: &[u8]) -> Result<()> {
        self.ensure_connected()?;
        if self.token.is_none() {
            self.authenticate()
                .context("authentication handshake failed")?;
        }
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
        self.ensure_connected()?;
        self.conn.as_mut().unwrap().subscribe(handle)?;
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
        for &h in DEFAULT_SUBSCRIBE_HANDLES {
            let _ = self.subscribe(h);
        }

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

    fn volume(&mut self) -> Result<u8> {
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
    fn write_volume(&mut self, cur: u8, level: u8) -> Result<u8> {
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
    fn set_volume(&mut self, level: u8) -> Result<u8> {
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

    fn read_byte(&mut self, handle: u16) -> Result<u8> {
        let v = self.read(handle)?;
        v.first()
            .copied()
            .ok_or_else(|| anyhow!("empty value for handle 0x{handle:04x}"))
    }

    fn update_byte(&mut self, handle: u16, val: u8) -> Result<u8> {
        self.drain();
        self.write_cmd(handle, &[val])?;
        let _ = self.wait_notify(handle, Duration::from_millis(500));
        self.read_byte(handle)
    }

    fn get_bool(&mut self, handle: u16) -> Result<bool> {
        let v = self.read(handle)?;
        Ok(v.first().copied().unwrap_or(0) != 0)
    }

    fn set_bool(&mut self, handle: u16, on: bool) -> Result<bool> {
        self.drain();
        self.write_cmd(handle, &[if on { 1 } else { 0 }])?;
        let _ = self.wait_notify(handle, Duration::from_millis(500));
        self.get_bool(handle)
    }

    fn toggle_bool(&mut self, handle: u16) -> Result<bool> {
        let cur = self.get_bool(handle)?;
        self.set_bool(handle, !cur)
    }

    fn read_level(&mut self, handle: u16) -> Result<i8> {
        let v = self.read_byte(handle)?;
        Ok(v as i8)
    }

    fn write_level(&mut self, handle: u16, level: i8) -> Result<i8> {
        if !(-10..=10).contains(&level) {
            bail!("level {level} out of range (expected -10 to +10)");
        }
        let res = self.update_byte(handle, level as u8)?;
        Ok(res as i8)
    }

    fn get_enum<T: VizioEnum>(&mut self, handle: u16) -> Result<T> {
        let byte = self.read_byte(handle)?;
        T::from_u8(byte).ok_or_else(|| anyhow!("unknown value byte: {byte}"))
    }

    fn set_enum<T: VizioEnum>(&mut self, handle: u16, val: T) -> Result<T> {
        let byte = self.update_byte(handle, val.to_u8())?;
        T::from_u8(byte).ok_or_else(|| anyhow!("unknown value byte: {byte}"))
    }

    fn set_bounded_u8(&mut self, name: &str, handle: u16, val: u8, max: u8) -> Result<u8> {
        if val > max {
            bail!("{name} {val} out of range (expected 0 to {max})");
        }
        self.update_byte(handle, val)
    }

    fn snapshot_info(&mut self) -> Result<HashMap<u16, Vec<u8>>> {
        self.ensure_connected()?;
        let mut map = HashMap::new();
        for &(h, _, _) in INFO_FIELDS {
            if let Ok(v) = self.read(h) {
                map.insert(h, v);
            }
        }
        if map.is_empty() {
            bail!("failed to read any info fields (soundbar unreachable)");
        }
        Ok(map)
    }

    fn snapshot_all(&mut self) -> Result<HashMap<u16, Vec<u8>>> {
        self.ensure_connected()?;
        let chars = self.conn.as_ref().unwrap().characteristics().to_vec();
        let mut map = HashMap::new();
        for c in &chars {
            if c.props.can_read() {
                if let Ok(v) = self.read(c.handle) {
                    map.insert(c.handle, v);
                }
            }
        }
        if map.is_empty() {
            bail!("failed to read any characteristics (soundbar unreachable)");
        }
        Ok(map)
    }

    fn subscribe_all_notify(&mut self) -> Result<usize> {
        self.ensure_connected()?;
        let handles: Vec<u16> = self
            .conn
            .as_ref()
            .unwrap()
            .characteristics()
            .iter()
            .filter(|c| c.props.can_notify())
            .map(|c| c.handle)
            .collect();
        let mut count = 0;
        for h in handles {
            if self.subscribe(h).is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    fn print_info(&mut self) -> Result<()> {
        self.ensure_connected()?;
        let info = self.conn.as_ref().unwrap().device_info()?;
        println!("Address      : {}", bt::fmt_addr(info.address));
        println!("Device name  : {}", info.name);
        println!("Connected    : {}", info.connected);
        if let Some(paired) = info.paired {
            println!("OS paired    : {paired}");
        }
        if let Some(t) = &self.token {
            println!("Session token: {}", printable(t));
        }
        let chars = self.conn.as_ref().unwrap().characteristics().to_vec();
        // Standard GATT strings (Device Information service etc.), if exposed.
        for c in &chars {
            let label = match c.uuid.short_uuid() {
                Some(0x2A24) => "Model",
                Some(0x2A25) => "Serial",
                Some(0x2A26) => "Firmware",
                Some(0x2A27) => "Hardware",
                Some(0x2A28) => "Software",
                Some(0x2A29) => "Manufacturer",
                _ => continue,
            };
            if let Ok(v) = self.read(c.handle) {
                println!(
                    "{label:<17}: {}",
                    String::from_utf8_lossy(&v).trim_end_matches('\0')
                );
            }
        }
        for &(h, label, fmt) in INFO_FIELDS {
            if !chars.iter().any(|c| c.handle == h) {
                continue;
            }
            match self.read(h) {
                Ok(v) => {
                    let val = format_setting_value(fmt, h, &v, true);
                    println!("{label:<17}: {val}");
                }
                Err(e) => println!("{label:<17}: <{e}>"),
            }
        }
        Ok(())
    }

    fn dump(&mut self) -> Result<()> {
        self.ensure_connected()?;
        let conn = self.conn.as_ref().unwrap();
        println!(
            "handle offset: {:+} (AttributeHandle -> ATT value handle)",
            conn.handle_offset()
        );
        let mut last_svc = None;
        let mut chars = conn.characteristics().to_vec();
        chars.sort_by_key(|c| c.handle);
        for c in chars {
            if last_svc != Some(c.service_uuid) {
                println!("\nService {}", c.service_uuid);
                last_svc = Some(c.service_uuid);
            }
            let mut line = format!("  0x{:04x}  {:<38} [{}]", c.handle, c.uuid, c.props);
            if c.props.can_read() {
                match self.read(c.handle) {
                    Ok(v) => line += &format!("  = {}  \"{}\"", hex(&v), printable(&v)),
                    Err(e) => line += &format!("  <{e}>"),
                }
            }
            println!("{line}");
        }
        Ok(())
    }

    fn close(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.close();
        }
    }
}

impl Drop for Soundbar {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_scan(cli: &Cli) -> Result<()> {
    println!("Paired devices:");
    for (name, addr, kind) in bt::paired_devices().unwrap_or_default() {
        println!("  {}  {:<8} {name}", bt::fmt_addr(addr), kind);
    }
    println!("\nScanning BLE for {}s...", cli.scan_secs);
    let mut seen: Vec<_> = bt::scan(cli.scan_secs, None)?.into_iter().collect();
    seen.sort_by_key(|(_, (_, rssi))| -rssi);
    for (addr, (name, rssi)) in seen {
        let mark = if name.to_lowercase().contains(&cli.name.to_lowercase()) {
            " <=="
        } else {
            ""
        };
        println!("  {}  {rssi:>4} dBm  {name}{mark}", bt::fmt_addr(addr));
    }
    Ok(())
}

fn cmd_pair(cli: &Cli) -> Result<()> {
    bt::pair(cli)
}

fn cmd_unpair(cli: &Cli) -> Result<()> {
    bt::unpair(cli)
}

fn apply_volume(sb: &mut Soundbar, action: &VolAction) -> Result<()> {
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

fn apply_mute(sb: &mut Soundbar, action: &MuteAction) -> Result<()> {
    let m = match action {
        MuteAction::Get => sb.get_bool(H_MUTE)?,
        MuteAction::On => sb.set_bool(H_MUTE, true)?,
        MuteAction::Off => sb.set_bool(H_MUTE, false)?,
        MuteAction::Toggle => sb.toggle_bool(H_MUTE)?,
    };
    println!("Mute: {}", if m { "on" } else { "off" });
    Ok(())
}

fn apply_on_off(
    sb: &mut Soundbar,
    name: &str,
    handle: u16,
    action: Option<&OnOffAction>,
) -> Result<()> {
    let action = action.unwrap_or(&OnOffAction::Get);
    let on = match action {
        OnOffAction::Get => sb.get_bool(handle)?,
        OnOffAction::On => sb.set_bool(handle, true)?,
        OnOffAction::Off => sb.set_bool(handle, false)?,
        OnOffAction::Toggle => sb.toggle_bool(handle)?,
    };
    println!("{name}: {}", if on { "on" } else { "off" });
    Ok(())
}

fn apply_enum<T: VizioEnum + std::fmt::Display>(
    sb: &mut Soundbar,
    name: &str,
    handle: u16,
    mode: Option<T>,
) -> Result<()> {
    let res = match mode {
        Some(m) => sb.set_enum(handle, m)?,
        None => sb.get_enum(handle)?,
    };
    println!("{name}: {res} ({})", res.to_u8());
    Ok(())
}

fn apply_bounded_u8(
    sb: &mut Soundbar,
    name: &str,
    handle: u16,
    val: Option<u8>,
    max: u8,
) -> Result<()> {
    let res = match val {
        Some(v) => sb.set_bounded_u8(name, handle, v, max)?,
        None => sb.read_byte(handle)?,
    };
    println!("{name}: {res}");
    Ok(())
}

fn apply_level(sb: &mut Soundbar, name: &str, handle: u16, level: Option<i8>) -> Result<()> {
    let val = match level {
        Some(l) => sb.write_level(handle, l)?,
        None => sb.read_level(handle)?,
    };
    println!("{name}: {}", fmt_level(val));
    Ok(())
}

fn apply_factory_reset(sb: &mut Soundbar, yes_really: bool) -> Result<()> {
    if !yes_really {
        print!(
            "Are you sure you want to factory reset the soundbar? This will wipe all settings. [y/N]: "
        );
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let trimmed = answer.trim();
        if !trimmed.eq_ignore_ascii_case("y") && !trimmed.eq_ignore_ascii_case("yes") {
            println!("Aborted.");
            return Ok(());
        }
    }
    sb.write_cmd(H_FACTORY_RESET, &[0])?;
    println!("Factory reset initiated. The soundbar will restore defaults and reboot.");
    Ok(())
}

fn apply_levels(sb: &mut Soundbar, target: Option<&LevelCmd>) -> Result<()> {
    match target {
        Some(LevelCmd::Bass { level }) => apply_level(sb, "Bass", H_LEVEL_BASS, *level),
        Some(LevelCmd::Treble { level }) => apply_level(sb, "Treble", H_LEVEL_TREBLE, *level),
        Some(LevelCmd::Center { level }) => apply_level(sb, "Center", H_LEVEL_CENTER, *level),
        Some(LevelCmd::Subwoofer { level }) => {
            apply_level(sb, "Subwoofer", H_LEVEL_SUBWOOFER, *level)
        }
        Some(LevelCmd::Surround { level }) => {
            apply_level(sb, "Surround Level", H_LEVEL_SURROUND, *level)
        }
        Some(LevelCmd::Balance { level }) => {
            apply_level(sb, "Surround Balance", H_LEVEL_BALANCE, *level)
        }
        None => {
            println!("Audio Levels (-10 to +10):");
            for &(h, name) in &[
                (H_LEVEL_BASS, "Bass"),
                (H_LEVEL_TREBLE, "Treble"),
                (H_LEVEL_CENTER, "Center"),
                (H_LEVEL_SUBWOOFER, "Subwoofer"),
                (H_LEVEL_SURROUND, "Surround Level"),
                (H_LEVEL_BALANCE, "Surround Balance"),
            ] {
                let lvl = sb.read_level(h)?;
                println!("  {name:<16}: {}", fmt_level(lvl));
            }
            Ok(())
        }
    }
}

fn cmd_monitor(sb: &mut Soundbar) -> Result<()> {
    sb.ensure_connected()?;
    let handles: Vec<u16> = sb
        .conn
        .as_ref()
        .unwrap()
        .characteristics()
        .iter()
        .filter(|c| c.props.can_notify())
        .map(|c| c.handle)
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

fn diff_info(sb: &mut Soundbar, snap_info: &mut Option<HashMap<u16, Vec<u8>>>) -> Result<()> {
    let cur = sb.snapshot_info()?;
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
                    Some(b) => {
                        if let Some((_, fmt)) = field_by_handle(h) {
                            format_setting_value(fmt, h, b, true)
                        } else if b.len() == 1 {
                            format!("{} (0x{:02x})", b[0], b[0])
                        } else {
                            hex(b)
                        }
                    }
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
    Ok(())
}

fn diff_all(sb: &mut Soundbar, snap_all: &mut Option<HashMap<u16, Vec<u8>>>) -> Result<()> {
    let cur = sb.snapshot_all()?;
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
                    if let Some((_, fmt)) = field_by_handle(h) {
                        format_setting_value(fmt, h, b, true)
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
    Ok(())
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
        DeviceCmd::Input { mode } => apply_enum(sb, "Input Source", H_INPUT, *mode),
        DeviceCmd::Eq { mode } => apply_enum(sb, "EQ Mode", H_EQ, *mode),
        DeviceCmd::Surround { mode } => apply_enum(sb, "Surround Mode", H_SURROUND, *mode),
        DeviceCmd::Position { mode } => apply_enum(sb, "Elevation Mode", H_ELEVATION_MODE, *mode),
        DeviceCmd::Color { color } => apply_enum(sb, "Speaker Color", H_SPEAKER_COLOR, *color),
        DeviceCmd::PowerLight { action } => {
            apply_on_off(sb, "Power Light", H_POWER_LIGHT, action.as_ref())
        }
        DeviceCmd::AutoHdmi { action } => {
            apply_on_off(sb, "Auto HDMI", H_AUTO_HDMI, action.as_ref())
        }
        DeviceCmd::AvDelay { delay } => apply_bounded_u8(sb, "AV Delay", H_AV_DELAY, *delay, 8),
        DeviceCmd::Dialog { level } => apply_bounded_u8(sb, "Clear Dialog", H_DIALOG, *level, 10),
        DeviceCmd::VirtualX { action } => {
            apply_on_off(sb, "Virtual:X", H_VIRTUAL_X, action.as_ref())
        }
        DeviceCmd::TruVolume { action } => {
            apply_on_off(sb, "TruVolume", H_TRUVOLUME, action.as_ref())
        }
        DeviceCmd::NightMode { action } => {
            apply_on_off(sb, "Night Mode", H_NIGHT_MODE, action.as_ref())
        }
        DeviceCmd::Levels { target } => apply_levels(sb, target.as_ref()),
        DeviceCmd::Bass { level } => apply_level(sb, "Bass", H_LEVEL_BASS, *level),
        DeviceCmd::Treble { level } => apply_level(sb, "Treble", H_LEVEL_TREBLE, *level),
        DeviceCmd::Center { level } => apply_level(sb, "Center", H_LEVEL_CENTER, *level),
        DeviceCmd::Subwoofer { level } => apply_level(sb, "Subwoofer", H_LEVEL_SUBWOOFER, *level),
        DeviceCmd::SurroundLevel { level } => {
            apply_level(sb, "Surround Level", H_LEVEL_SURROUND, *level)
        }
        DeviceCmd::SurroundBalance { balance } => {
            apply_level(sb, "Surround Balance", H_LEVEL_BALANCE, *balance)
        }
        DeviceCmd::FactoryReset { yes_really } => apply_factory_reset(sb, *yes_really),
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
         in / input [bluetooth|hdmi-earc], eq [movie|music|game|direct], surr [off|on|dual|front], \
         elev [up|wide|auto], color [off|rainbow|...], levels, bass [n], treble [n], \
         center [n], sub [n], dialog [0-10], vx [on|off], truvol [on|off], night [on|off], \
         power-light [on|off], hdmi [on|off], delay [0-8], reset [--yes-really], d / diff [all], snap, \
         disc / disconnect, rec / reconnect, watch, info, dump, read <h>, write <h> <hex>, q (or 'help')"
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
            ShellCmd::Disconnect => {
                sb.disconnect();
                Ok(())
            }
            ShellCmd::Reconnect => sb.reconnect(),
            ShellCmd::Watch => {
                let count = sb.subscribe_all_notify()?;
                println!(
                    "Subscribed to {count} characteristics. Notifications will print live at prompt."
                );
                Ok(())
            }
            ShellCmd::Snap => {
                let info = sb.snapshot_info()?;
                let all = sb.snapshot_all()?;
                println!(
                    "Snapshots updated as new baseline ({} info fields, {} handles).",
                    info.len(),
                    all.len()
                );
                snap_info = Some(info);
                snap_all = Some(all);
                Ok(())
            }
            ShellCmd::Diff { all, target } => {
                let is_all = all || target.as_deref() == Some("all");
                if is_all {
                    diff_all(sb, &mut snap_all)
                } else {
                    diff_info(sb, &mut snap_info)
                }
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
        bt::print_platform_error_hint(&e);
        std::process::exit(1);
    }
}
