"""Sensor entities for VIZIO Elevate BLE soundbar."""

from __future__ import annotations

from homeassistant.components.sensor import SensorEntity
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import VizioConfigEntry
from .client import H_AUDIO_FORMAT
from .entity import VizioElevateEntity


async def async_setup_entry(
    hass: HomeAssistant, entry: VizioConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    async_add_entities([VizioAudioFormatSensor(entry)])


class VizioAudioFormatSensor(VizioElevateEntity, SensorEntity):
    """Sensor for current audio format (e.g. Dolby Atmos, DTS:X, Stereo PCM)."""

    _attr_icon = "mdi:audio-video"
    _attr_translation_key = "audio_format"

    def __init__(self, entry: VizioConfigEntry) -> None:
        super().__init__(entry, key="audio_format")
        self._attr_name = "Audio Format"

    @property
    def native_value(self) -> str | None:
        return self._client.state.get(H_AUDIO_FORMAT)
