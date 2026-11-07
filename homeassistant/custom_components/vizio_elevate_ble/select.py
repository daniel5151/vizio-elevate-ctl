"""Select entities for VIZIO Elevate BLE soundbar."""

from __future__ import annotations

from homeassistant.components.select import SelectEntity
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import VizioConfigEntry
from .client import ElevationMode, SpeakerColor, SurroundMode
from .entity import VizioElevateEntity


async def async_setup_entry(
    hass: HomeAssistant, entry: VizioConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    async_add_entities([
        VizioElevationSelect(entry),
        VizioSurroundSelect(entry),
        VizioSpeakerColorSelect(entry),
    ])


class VizioElevationSelect(VizioElevateEntity, SelectEntity):
    """Select entity for elevation speaker position (Up, Wide, Auto)."""

    _attr_icon = "mdi:arrow-up-down"
    _attr_options = [mode.display_name for mode in ElevationMode]

    def __init__(self, entry: VizioConfigEntry) -> None:
        super().__init__(entry, key="elevation_mode")
        self._attr_name = "Elevation Mode"

    @property
    def current_option(self) -> str | None:
        mode = self._client.elevation_mode
        return mode.display_name if mode is not None else None

    async def async_select_option(self, option: str) -> None:
        await self._client.set_elevation(option)


class VizioSurroundSelect(VizioElevateEntity, SelectEntity):
    """Select entity for surround sound mode."""

    _attr_icon = "mdi:surround-sound"
    _attr_options = [mode.display_name for mode in SurroundMode]

    def __init__(self, entry: VizioConfigEntry) -> None:
        super().__init__(entry, key="surround_mode")
        self._attr_name = "Surround Mode"

    @property
    def current_option(self) -> str | None:
        mode = self._client.surround_mode
        return mode.display_name if mode is not None else None

    async def async_select_option(self, option: str) -> None:
        await self._client.set_surround(option)


class VizioSpeakerColorSelect(VizioElevateEntity, SelectEntity):
    """Select entity for elevate speaker LED color."""

    _attr_icon = "mdi:palette"
    _attr_options = [color.display_name for color in SpeakerColor]

    def __init__(self, entry: VizioConfigEntry) -> None:
        super().__init__(entry, key="speaker_color")
        self._attr_name = "Speaker Color"

    @property
    def current_option(self) -> str | None:
        color = self._client.speaker_color
        return color.display_name if color is not None else None

    async def async_select_option(self, option: str) -> None:
        await self._client.set_speaker_color(option)
