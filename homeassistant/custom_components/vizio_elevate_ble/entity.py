"""Base entity for VIZIO Elevate BLE integration."""

from __future__ import annotations

from homeassistant.components import bluetooth
from homeassistant.components.bluetooth import (
    BluetoothCallbackMatcher,
    BluetoothChange,
    BluetoothScanningMode,
    BluetoothServiceInfoBleak,
)
from homeassistant.const import CONF_ADDRESS
from homeassistant.core import callback
from homeassistant.helpers import device_registry as dr
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.entity import Entity

from . import VizioConfigEntry
from .client import VizioElevateClient
from .const import DOMAIN


class VizioElevateEntity(Entity):
    """Base class for all VIZIO Elevate entities."""

    _attr_has_entity_name = True
    _attr_should_poll = False

    def __init__(self, entry: VizioConfigEntry, key: str) -> None:
        """Initialize the entity."""
        self._entry = entry
        self._client: VizioElevateClient = entry.runtime_data
        self._address: str = entry.data[CONF_ADDRESS]
        self._attr_unique_id = f"{self._address}_{key}"
        self._seen = False

    @property
    def device_info(self) -> DeviceInfo:
        return DeviceInfo(
            identifiers={(DOMAIN, self._address)},
            connections={(dr.CONNECTION_BLUETOOTH, self._address)},
            name=self._entry.title,
            manufacturer="VIZIO",
            model=self._client.model or "Elevate Soundbar",
            sw_version=self._client.firmware_version,
            serial_number=self._client.serial_number,
        )

    async def async_added_to_hass(self) -> None:
        """Handle entity added to Home Assistant."""
        self._seen = bluetooth.async_address_present(self.hass, self._address, connectable=True)
        self.async_on_remove(self._client.add_listener(self._on_client_update))
        self.async_on_remove(
            bluetooth.async_register_callback(
                self.hass,
                self._on_advertisement,
                BluetoothCallbackMatcher(address=self._address, connectable=True),
                BluetoothScanningMode.ACTIVE,
            )
        )
        self.async_on_remove(
            bluetooth.async_track_unavailable(
                self.hass, self._on_unavailable, self._address, connectable=True
            )
        )

    @callback
    def _on_client_update(self) -> None:
        self.async_write_ha_state()

    @callback
    def _on_advertisement(self, info: BluetoothServiceInfoBleak, change: BluetoothChange) -> None:
        if not self._seen:
            self._seen = True
            self.async_write_ha_state()

    @callback
    def _on_unavailable(self, info: BluetoothServiceInfoBleak) -> None:
        self._seen = False
        self.async_write_ha_state()

    @property
    def available(self) -> bool:
        return self._seen or self._client.connected

    async def async_update(self) -> None:
        """Manual update requested via homeassistant.update_entity service."""
        try:
            await self._client.refresh()
        except Exception:
            pass
