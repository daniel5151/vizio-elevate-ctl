"""Config flow: automatic Bluetooth discovery plus manual pick-from-list."""

from __future__ import annotations

import logging
from typing import Any

import voluptuous as vol

from homeassistant.components import bluetooth
from homeassistant.components.bluetooth import BluetoothServiceInfoBleak
from homeassistant.config_entries import ConfigEntry, ConfigFlow, ConfigFlowResult, OptionsFlow
from homeassistant.core import callback
from homeassistant.const import CONF_ADDRESS

from .client import VizioElevateClient, VizioError
from .const import (
    CONF_DISCONNECT_IDLE,
    CONF_MAX_VOLUME,
    DEFAULT_DISCONNECT_IDLE,
    DEFAULT_MAX_VOLUME,
    DOMAIN,
)

_LOGGER = logging.getLogger(__name__)


def _is_vizio(info: BluetoothServiceInfoBleak) -> bool:
    return "vizio mobile app" in (info.name or "").lower()


class VizioElevateConfigFlow(ConfigFlow, domain=DOMAIN):
    VERSION = 1

    def __init__(self) -> None:
        self._discovery: BluetoothServiceInfoBleak | None = None
        self._discovered: dict[str, BluetoothServiceInfoBleak] = {}

    @staticmethod
    @callback
    def async_get_options_flow(config_entry: ConfigEntry) -> OptionsFlow:
        return VizioElevateOptionsFlow()

    async def _async_probe(self, address: str) -> str | None:
        """Try a real connect + auth handshake. Returns an error string or None."""
        client = VizioElevateClient(
            lambda: bluetooth.async_ble_device_from_address(self.hass, address, connectable=True),
            idle_timeout=None,
        )
        try:
            await client.connect()
        except VizioError as err:
            _LOGGER.warning("Setup probe of %s failed: %s", address, err)
            return str(err)
        finally:
            await client.close()
        return None

    # -- triggered by the manifest "bluetooth" matcher --------------------------

    async def async_step_bluetooth(
        self, discovery_info: BluetoothServiceInfoBleak
    ) -> ConfigFlowResult:
        await self.async_set_unique_id(discovery_info.address)
        self._abort_if_unique_id_configured()
        self._discovery = discovery_info
        self.context["title_placeholders"] = {"name": discovery_info.name}
        return await self.async_step_bluetooth_confirm()

    async def async_step_bluetooth_confirm(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        assert self._discovery is not None
        errors: dict[str, str] = {}
        last_error = ""
        if user_input is not None:
            if (err := await self._async_probe(self._discovery.address)) is None:
                return self.async_create_entry(
                    title=self._discovery.name,
                    data={CONF_ADDRESS: self._discovery.address},
                )
            errors["base"] = "cannot_connect"
            last_error = f"Details: {err}"
        self._set_confirm_only()
        return self.async_show_form(
            step_id="bluetooth_confirm",
            description_placeholders={"name": self._discovery.name, "error": last_error},
            errors=errors,
        )

    # -- manual ----------------------------------------------------------------

    async def async_step_user(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        errors: dict[str, str] = {}
        last_error = ""
        if user_input is not None:
            address = user_input[CONF_ADDRESS]
            await self.async_set_unique_id(address, raise_on_progress=False)
            self._abort_if_unique_id_configured()
            if (err := await self._async_probe(address)) is None:
                info = self._discovered.get(address)
                return self.async_create_entry(
                    title=info.name if info else address,
                    data={CONF_ADDRESS: address},
                )
            errors["base"] = "cannot_connect"
            last_error = f"Details: {err}"

        current = self._async_current_ids()
        for info in bluetooth.async_discovered_service_info(self.hass, connectable=True):
            if info.address not in current and _is_vizio(info):
                self._discovered[info.address] = info
        if not self._discovered:
            return self.async_abort(reason="no_devices_found")

        return self.async_show_form(
            step_id="user",
            data_schema=vol.Schema(
                {
                    vol.Required(CONF_ADDRESS): vol.In(
                        {a: f"{i.name} ({a})" for a, i in self._discovered.items()}
                    )
                }
            ),
            errors=errors,
            description_placeholders={"error": last_error},
        )


class VizioElevateOptionsFlow(OptionsFlow):
    async def async_step_init(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        if user_input is not None:
            return self.async_create_entry(data=user_input)
        disconnect_current = self.config_entry.options.get(
            CONF_DISCONNECT_IDLE, DEFAULT_DISCONNECT_IDLE
        )
        max_vol_current = self.config_entry.options.get(
            CONF_MAX_VOLUME, DEFAULT_MAX_VOLUME
        )
        return self.async_show_form(
            step_id="init",
            data_schema=vol.Schema(
                {
                    vol.Required(CONF_DISCONNECT_IDLE, default=disconnect_current): bool,
                    vol.Required(CONF_MAX_VOLUME, default=max_vol_current): vol.All(
                        vol.Coerce(int), vol.Range(min=1, max=100)
                    ),
                }
            ),
        )
