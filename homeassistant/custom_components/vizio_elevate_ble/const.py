"""Constants for the VIZIO Elevate BLE integration."""

from homeassistant.const import Platform

DOMAIN = "vizio_elevate_ble"

PLATFORMS = [
    Platform.MEDIA_PLAYER,
    Platform.SENSOR,
    Platform.SELECT,
    Platform.SWITCH,
    Platform.NUMBER,
]

# Options
CONF_DISCONNECT_IDLE = "disconnect_when_idle"
DEFAULT_DISCONNECT_IDLE = False

CONF_MAX_VOLUME = "max_volume"
DEFAULT_MAX_VOLUME = 100

# Seconds of inactivity before dropping the BLE link (only if disconnect_when_idle is enabled)
IDLE_TIMEOUT = 120.0
