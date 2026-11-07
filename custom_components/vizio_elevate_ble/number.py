"""Number entities for VIZIO Elevate BLE soundbar."""

from __future__ import annotations

from dataclasses import dataclass

from homeassistant.components.number import NumberEntity, NumberEntityDescription, NumberMode
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import VizioConfigEntry
from .client import (
    H_AV_DELAY,
    H_DIALOG,
    H_LEVEL_BALANCE,
    H_LEVEL_BASS,
    H_LEVEL_CENTER,
    H_LEVEL_SUBWOOFER,
    H_LEVEL_SURROUND,
    H_LEVEL_TREBLE,
)
from .entity import VizioElevateEntity


@dataclass(frozen=True, kw_only=True)
class VizioNumberDescription(NumberEntityDescription):
    """Description for VIZIO numeric level entities."""

    handle: int
    is_signed: bool = True


NUMBERS: tuple[VizioNumberDescription, ...] = (
    VizioNumberDescription(
        key="bass",
        name="Bass",
        icon="mdi:speaker",
        native_min_value=-10,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_LEVEL_BASS,
        is_signed=True,
    ),
    VizioNumberDescription(
        key="treble",
        name="Treble",
        icon="mdi:sine-wave",
        native_min_value=-10,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_LEVEL_TREBLE,
        is_signed=True,
    ),
    VizioNumberDescription(
        key="center",
        name="Center Level",
        icon="mdi:speaker-message",
        native_min_value=-10,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_LEVEL_CENTER,
        is_signed=True,
    ),
    VizioNumberDescription(
        key="subwoofer",
        name="Subwoofer Level",
        icon="mdi:speaker",
        native_min_value=-10,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_LEVEL_SUBWOOFER,
        is_signed=True,
    ),
    VizioNumberDescription(
        key="surround_level",
        name="Surround Level",
        icon="mdi:surround-sound",
        native_min_value=-10,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_LEVEL_SURROUND,
        is_signed=True,
    ),
    VizioNumberDescription(
        key="surround_balance",
        name="Surround Balance",
        icon="mdi:scale-balance",
        native_min_value=-10,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_LEVEL_BALANCE,
        is_signed=True,
    ),
    VizioNumberDescription(
        key="clear_dialog",
        name="Clear Dialog",
        icon="mdi:account-voice",
        native_min_value=0,
        native_max_value=10,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_DIALOG,
        is_signed=False,
    ),
    VizioNumberDescription(
        key="av_delay",
        name="AV Lip Sync Delay",
        icon="mdi:timer-sand",
        native_min_value=0,
        native_max_value=8,
        native_step=1,
        mode=NumberMode.SLIDER,
        handle=H_AV_DELAY,
        is_signed=False,
    ),
)


async def async_setup_entry(
    hass: HomeAssistant, entry: VizioConfigEntry, async_add_entities: AddEntitiesCallback
) -> None:
    async_add_entities([VizioElevateNumber(entry, desc) for desc in NUMBERS])


class VizioElevateNumber(VizioElevateEntity, NumberEntity):
    """Number entity for VIZIO audio level controls."""

    entity_description: VizioNumberDescription

    def __init__(self, entry: VizioConfigEntry, description: VizioNumberDescription) -> None:
        super().__init__(entry, key=description.key)
        self.entity_description = description
        self._attr_name = description.name

    @property
    def native_value(self) -> float | None:
        val = self._client.state.get(self.entity_description.handle)
        return float(val) if val is not None else None

    async def async_set_native_value(self, value: float) -> None:
        int_val = round(value)
        if self.entity_description.is_signed:
            await self._client.set_level(self.entity_description.handle, int_val)
        else:
            await self._client.set_bounded_u8(
                self.entity_description.handle,
                int_val,
                int(self.entity_description.native_max_value),
            )
