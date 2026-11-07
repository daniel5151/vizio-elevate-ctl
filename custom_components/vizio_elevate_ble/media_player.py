"""Media player entity for VIZIO Elevate BLE soundbar."""

from __future__ import annotations

from typing import Any

from homeassistant.components.media_player import (
    MediaPlayerDeviceClass,
    MediaPlayerEntity,
    MediaPlayerEntityFeature,
    MediaPlayerState,
)
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import VizioConfigEntry
from .client import EqMode, InputMode, MAX_VOLUME
from .entity import VizioElevateEntity


async def async_setup_entry(
    hass: HomeAssistant, entry: VizioConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    async_add_entities([VizioElevateMediaPlayer(entry)])


class VizioElevateMediaPlayer(VizioElevateEntity, MediaPlayerEntity):
    """Media player entity supporting volume, mute, source, and EQ modes."""

    _attr_device_class = MediaPlayerDeviceClass.SPEAKER
    _attr_name = None  # Primary device entity inherits the device name
    _attr_supported_features = (
        MediaPlayerEntityFeature.VOLUME_SET
        | MediaPlayerEntityFeature.VOLUME_STEP
        | MediaPlayerEntityFeature.VOLUME_MUTE
        | MediaPlayerEntityFeature.SELECT_SOURCE
        | MediaPlayerEntityFeature.SELECT_SOUND_MODE
    )

    def __init__(self, entry: VizioConfigEntry) -> None:
        super().__init__(entry, key="media_player")
        # Keep unique_id matching the original address so existing HA setups don't create duplicate entities
        self._attr_unique_id = self._address

    @property
    def state(self) -> MediaPlayerState:
        return MediaPlayerState.ON if self.available else MediaPlayerState.OFF

    # -- volume & mute ---------------------------------------------------------

    @property
    def _effective_max_volume(self) -> int:
        return self._client.max_volume or MAX_VOLUME

    @property
    def volume_level(self) -> float | None:
        v = self._client.volume
        if v is None:
            return None
        return min(1.0, max(0.0, v / self._effective_max_volume))

    async def async_set_volume_level(self, volume: float) -> None:
        await self._client.set_volume(round(volume * self._effective_max_volume))

    async def async_volume_up(self) -> None:
        await self._client.step_volume(+1)

    async def async_volume_down(self) -> None:
        await self._client.step_volume(-1)

    @property
    def extra_state_attributes(self) -> dict[str, Any]:
        return {
            "soundbar_volume": self._client.volume,
            "max_volume_limit": self._effective_max_volume,
        }

    @property
    def is_volume_muted(self) -> bool | None:
        return self._client.is_muted

    async def async_mute_volume(self, mute: bool) -> None:
        await self._client.set_mute(mute)

    # -- source / input --------------------------------------------------------

    @property
    def source(self) -> str | None:
        mode = self._client.input_mode
        return mode.display_name if mode is not None else None

    @property
    def source_list(self) -> list[str]:
        return [mode.display_name for mode in InputMode]

    async def async_select_source(self, source: str) -> None:
        await self._client.set_input(source)

    # -- sound mode / EQ -------------------------------------------------------

    @property
    def sound_mode(self) -> str | None:
        mode = self._client.eq_mode
        return mode.display_name if mode is not None else None

    @property
    def sound_mode_list(self) -> list[str]:
        return [mode.display_name for mode in EqMode]

    async def async_select_sound_mode(self, sound_mode: str) -> None:
        await self._client.set_eq(sound_mode)
