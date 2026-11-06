//! Bluetooth LE hardware abstraction layer (HAL).

pub mod types;
pub use types::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
pub use unsupported::*;
