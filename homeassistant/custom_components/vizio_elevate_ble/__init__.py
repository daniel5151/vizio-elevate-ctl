"""VIZIO Elevate soundbar (Bluetooth LE) integration."""

from __future__ import annotations

from homeassistant.components import bluetooth
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import CONF_ADDRESS
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import ConfigEntryNotReady

from .client import VizioElevateClient, VizioError
from .const import (
    CONF_DISCONNECT_IDLE,
    CONF_MAX_VOLUME,
    DEFAULT_DISCONNECT_IDLE,
    DEFAULT_MAX_VOLUME,
    IDLE_TIMEOUT,
    PLATFORMS,
)

type VizioConfigEntry = ConfigEntry[VizioElevateClient]


async def async_setup_entry(hass: HomeAssistant, entry: VizioConfigEntry) -> bool:
    address: str = entry.data[CONF_ADDRESS]

    def get_ble_device():
        return bluetooth.async_ble_device_from_address(hass, address, connectable=True)

    if get_ble_device() is None:
        raise ConfigEntryNotReady(f"{address} not seen by any Bluetooth adapter/proxy yet")

    disconnect_idle = entry.options.get(CONF_DISCONNECT_IDLE, DEFAULT_DISCONNECT_IDLE)
    max_volume = entry.options.get(CONF_MAX_VOLUME, DEFAULT_MAX_VOLUME)
    client = VizioElevateClient(
        get_ble_device,
        name=entry.title,
        max_volume=max_volume,
        idle_timeout=IDLE_TIMEOUT if disconnect_idle else None,
    )
    entry.runtime_data = client
    entry.async_on_unload(client.close)
    entry.async_on_unload(entry.add_update_listener(_async_options_updated))

    # Best-effort initial read. If the soundbar is off we still load the entry;
    # the entity just shows unavailable until it advertises again.
    try:
        await client.connect()
    except VizioError:
        pass

    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    return True


async def _async_options_updated(hass: HomeAssistant, entry: VizioConfigEntry) -> None:
    """Reload so the new idle behaviour takes effect."""
    await hass.config_entries.async_reload(entry.entry_id)


async def async_unload_entry(hass: HomeAssistant, entry: VizioConfigEntry) -> bool:
    return await hass.config_entries.async_unload_platforms(entry, PLATFORMS)
