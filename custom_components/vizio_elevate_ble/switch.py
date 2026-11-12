"""Switch entities for VIZIO Elevate BLE soundbar."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from homeassistant.components.switch import SwitchEntity, SwitchEntityDescription
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import VizioConfigEntry
from .client import (
    H_AUTO_HDMI,
    H_NIGHT_MODE,
    H_POWER_LIGHT,
    H_TRUVOLUME,
    H_VIRTUAL_X,
)
from .entity import VizioElevateEntity


@dataclass(frozen=True, kw_only=True)
class VizioSwitchDescription(SwitchEntityDescription):
    """Description for VIZIO toggle switch entities."""

    handle: int


SWITCHES: tuple[VizioSwitchDescription, ...] = (
    VizioSwitchDescription(
        key="virtual_x",
        translation_key="virtual_x",
        name="Virtual:X",
        icon="mdi:surround-sound",
        handle=H_VIRTUAL_X,
    ),
    VizioSwitchDescription(
        key="truvolume",
        translation_key="truvolume",
        name="TruVolume",
        icon="mdi:volume-vibrate",
        handle=H_TRUVOLUME,
    ),
    VizioSwitchDescription(
        key="night_mode",
        translation_key="night_mode",
        name="Night Mode",
        icon="mdi:weather-night",
        handle=H_NIGHT_MODE,
    ),
    VizioSwitchDescription(
        key="power_light",
        translation_key="power_light",
        name="Power Light",
        icon="mdi:led-on",
        handle=H_POWER_LIGHT,
        entity_category=EntityCategory.CONFIG,
    ),
    VizioSwitchDescription(
        key="auto_hdmi",
        translation_key="auto_hdmi",
        name="Auto HDMI",
        icon="mdi:hdmi-port",
        handle=H_AUTO_HDMI,
        entity_category=EntityCategory.CONFIG,
    ),
)


async def async_setup_entry(
    hass: HomeAssistant, entry: VizioConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    async_add_entities([VizioElevateSwitch(entry, desc) for desc in SWITCHES])


class VizioElevateSwitch(VizioElevateEntity, SwitchEntity):
    """Switch entity for VIZIO soundbar toggles."""

    entity_description: VizioSwitchDescription

    def __init__(self, entry: VizioConfigEntry, description: VizioSwitchDescription) -> None:
        super().__init__(entry, key=description.key)
        self.entity_description = description
        self._attr_name = description.name

    @property
    def is_on(self) -> bool | None:
        return self._client.state.get(self.entity_description.handle)

    async def async_turn_on(self, **kwargs: Any) -> None:
        await self._client.set_bool(self.entity_description.handle, True)

    async def async_turn_off(self, **kwargs: Any) -> None:
        await self._client.set_bool(self.entity_description.handle, False)
