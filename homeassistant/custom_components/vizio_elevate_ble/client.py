"""VIZIO Elevate BLE protocol client.

A pure-Python client for controlling the VIZIO Elevate Soundbar over Bluetooth LE.
Deliberately has NO Home Assistant imports so it can be tested standalone
(see tools/probe.py) or packaged for PyPI.

Protocol recap:
  1. write 0x00c2 = CLIENT_ID (with response); notify on 0x00bf
  2. read  0x00c2 -> 4-char challenge
  3. write 0x00c5 = challenge + ":" + AUTH_SUFFIX (with response)
  4. read  0x00c5 -> session token
  5. every control write is token + ":" + payload (write without response)
Volume is at 0x0079 and moves ONE step per write, so we walk to the target.
Other settings update in a single write and confirm via notifications.
"""

from __future__ import annotations

import asyncio
from enum import IntEnum
import logging
from typing import Any, Callable

from bleak.backends.characteristic import BleakGATTCharacteristic
from bleak.backends.device import BLEDevice
from bleak_retry_connector import BleakClientWithServiceCache, establish_connection

_LOGGER = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# GATT ATT Handles
# ---------------------------------------------------------------------------

H_MODEL = 0x004A
H_SERIAL = 0x004C
H_FIRMWARE = 0x004E
H_AUDIO_FORMAT = 0x0050
H_STATE_52 = 0x0052
H_DEVICE_NAME = 0x0054
H_DEVICE_TAG = 0x0056
H_POWER_LIGHT = 0x0064
H_MUTE = 0x006A
H_AV_DELAY = 0x006D
H_FACTORY_RESET = 0x0070
H_EQ = 0x0076
H_VOLUME = 0x0079
H_INPUT = 0x007C
H_SPEAKER_COLOR = 0x007F
H_STATE_82 = 0x0082
H_AUTO_HDMI = 0x0085
H_LEVEL_BASS = 0x0089
H_LEVEL_TREBLE = 0x008C
H_DIALOG = 0x008F
H_LEVEL_CENTER = 0x0092
H_LEVEL_SUBWOOFER = 0x0095
H_LEVEL_SURROUND = 0x0098
H_LEVEL_BALANCE = 0x009B
H_VIRTUAL_X = 0x00A2
H_TRUVOLUME = 0x00A5
H_NIGHT_MODE = 0x00A8
H_SURROUND = 0x00AB
H_ELEVATION_MODE = 0x00AE
H_AUTH_STATUS = 0x00BF
H_AUTH_CHALLENGE = 0x00C2
H_AUTH_RESPONSE = 0x00C5

# Known handles used to auto-detect handle offset across different BLE stacks
KNOWN_HANDLES = (
    H_MODEL,
    H_SERIAL,
    H_FIRMWARE,
    H_AUDIO_FORMAT,
    H_STATE_52,
    H_DEVICE_NAME,
    H_DEVICE_TAG,
    H_POWER_LIGHT,
    H_MUTE,
    H_AV_DELAY,
    H_FACTORY_RESET,
    H_EQ,
    H_VOLUME,
    H_INPUT,
    H_SPEAKER_COLOR,
    H_STATE_82,
    H_AUTO_HDMI,
    H_LEVEL_BASS,
    H_LEVEL_TREBLE,
    H_DIALOG,
    H_LEVEL_CENTER,
    H_LEVEL_SUBWOOFER,
    H_LEVEL_SURROUND,
    H_LEVEL_BALANCE,
    H_VIRTUAL_X,
    H_TRUVOLUME,
    H_NIGHT_MODE,
    H_SURROUND,
    H_ELEVATION_MODE,
    H_AUTH_STATUS,
    H_AUTH_CHALLENGE,
    H_AUTH_RESPONSE,
)

# Handles automatically subscribed for live notifications on connect
DEFAULT_SUBSCRIBE_HANDLES = (
    H_INPUT,
    H_VOLUME,
    H_MUTE,
    H_EQ,
    H_SURROUND,
    H_ELEVATION_MODE,
    H_SPEAKER_COLOR,
    H_POWER_LIGHT,
    H_AUTO_HDMI,
    H_AV_DELAY,
    H_LEVEL_BASS,
    H_LEVEL_TREBLE,
    H_LEVEL_CENTER,
    H_LEVEL_SUBWOOFER,
    H_LEVEL_SURROUND,
    H_LEVEL_BALANCE,
    H_DIALOG,
    H_VIRTUAL_X,
    H_TRUVOLUME,
    H_NIGHT_MODE,
)

CLIENT_ID = b"1:f4c46a1e-4e22-"
AUTH_SUFFIX = bytes(
    [0xF8, 0xB8, 0x18, 0x40, 0xC8, 0x20, 0x98, 0xE0, 0x08, 0x08, 0x20, 0x90, 0x38, 0x00]
)
MAX_VOLUME = 100
GATT_OP_TIMEOUT = 10.0


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------

class InputMode(IntEnum):
    BLUETOOTH = 1
    HDMI_EARC = 2

    @property
    def display_name(self) -> str:
        return {
            InputMode.BLUETOOTH: "Bluetooth",
            InputMode.HDMI_EARC: "HDMI-eARC",
        }[self]

    @classmethod
    def from_str(cls, name: str) -> InputMode | None:
        norm = name.lower().replace("-", "").replace("_", "").replace(" ", "")
        for mode in cls:
            if norm == mode.name.lower().replace("_", "") or norm == mode.display_name.lower().replace("-", "").replace(" ", ""):
                return mode
        if norm in ("bt", "bluetooth"):
            return cls.BLUETOOTH
        if norm in ("hdmi", "earc", "hdmiearc"):
            return cls.HDMI_EARC
        return None


class EqMode(IntEnum):
    MOVIE = 0
    MUSIC = 1
    GAME = 2
    DIRECT = 3

    @property
    def display_name(self) -> str:
        return {
            EqMode.MOVIE: "Movie",
            EqMode.MUSIC: "Music",
            EqMode.GAME: "Game",
            EqMode.DIRECT: "Direct",
        }[self]

    @classmethod
    def from_str(cls, name: str) -> EqMode | None:
        norm = name.lower().strip()
        for mode in cls:
            if norm == mode.display_name.lower():
                return mode
        return None


class SurroundMode(IntEnum):
    OFF = 0
    ON = 1
    DUAL = 2
    FRONT = 3

    @property
    def display_name(self) -> str:
        return {
            SurroundMode.OFF: "Off",
            SurroundMode.ON: "On",
            SurroundMode.DUAL: "Dual",
            SurroundMode.FRONT: "Front",
        }[self]

    @classmethod
    def from_str(cls, name: str) -> SurroundMode | None:
        norm = name.lower().strip()
        for mode in cls:
            if norm == mode.display_name.lower():
                return mode
        return None


class ElevationMode(IntEnum):
    UP = 0
    WIDE = 1
    AUTO = 2

    @property
    def display_name(self) -> str:
        return {
            ElevationMode.UP: "Up",
            ElevationMode.WIDE: "Wide",
            ElevationMode.AUTO: "Auto",
        }[self]

    @classmethod
    def from_str(cls, name: str) -> ElevationMode | None:
        norm = name.lower().strip()
        for mode in cls:
            if norm == mode.display_name.lower():
                return mode
        return None


class SpeakerColor(IntEnum):
    OFF = 0
    RAINBOW = 1
    LIGHT_BLUE = 2
    YELLOW = 3
    PURPLE = 4
    WHITE = 5
    CYAN = 6
    MAGENTA = 7

    @property
    def display_name(self) -> str:
        return {
            SpeakerColor.OFF: "Off",
            SpeakerColor.RAINBOW: "Rainbow",
            SpeakerColor.LIGHT_BLUE: "Light Blue",
            SpeakerColor.YELLOW: "Yellow",
            SpeakerColor.PURPLE: "Purple",
            SpeakerColor.WHITE: "White",
            SpeakerColor.CYAN: "Cyan",
            SpeakerColor.MAGENTA: "Magenta",
        }[self]

    @classmethod
    def from_str(cls, name: str) -> SpeakerColor | None:
        norm = name.lower().replace("-", "").replace("_", "").replace(" ", "")
        for mode in cls:
            if norm == mode.display_name.lower().replace(" ", ""):
                return mode
        return None


class VizioError(Exception):
    """Anything that goes wrong talking to the soundbar."""


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------

class VizioElevateClient:
    """Async client for VIZIO Elevate Soundbars."""

    def __init__(
        self,
        get_ble_device: Callable[[], BLEDevice | None],
        *,
        name: str = "VIZIO soundbar",
        max_volume: int = MAX_VOLUME,
        idle_timeout: float | None = 120.0,
    ) -> None:
        self._get_ble_device = get_ble_device
        self._name = name
        self.max_volume = min(max_volume, MAX_VOLUME)
        self._idle_timeout = idle_timeout

        self._client: BleakClientWithServiceCache | None = None
        self._chars: dict[int, BleakGATTCharacteristic] = {}
        self._offset = 0
        self._token: bytes | None = None
        self._stage = "idle"
        self._trace: list[str] = []
        self._subscribed: set[int] = set()
        self._notifications: asyncio.Queue[tuple[int, bytes]] = asyncio.Queue()

        self._conn_lock = asyncio.Lock()
        self._io_lock = asyncio.Lock()
        self._idle_handle: asyncio.TimerHandle | None = None
        self._idle_task: asyncio.Task | None = None
        self._walker: asyncio.Task | None = None
        self._target: int | None = None

        self._listeners: list[Callable[[], None]] = []

        # State storage
        self.state: dict[int, Any] = {}
        self.model: str | None = None
        self.serial_number: str | None = None
        self.firmware_version: str | None = None
        self.audio_format: str | None = None

    # -- properties ------------------------------------------------------------

    @property
    def connected(self) -> bool:
        return bool(self._client and self._client.is_connected and self._token)

    @property
    def volume(self) -> int | None:
        return self.state.get(H_VOLUME)

    @volume.setter
    def volume(self, val: int | None) -> None:
        if val is not None:
            self.state[H_VOLUME] = val

    @property
    def is_muted(self) -> bool | None:
        return self.state.get(H_MUTE)

    @property
    def input_mode(self) -> InputMode | None:
        val = self.state.get(H_INPUT)
        if val is not None:
            try:
                return InputMode(val)
            except ValueError:
                pass
        return None

    @property
    def eq_mode(self) -> EqMode | None:
        val = self.state.get(H_EQ)
        if val is not None:
            try:
                return EqMode(val)
            except ValueError:
                pass
        return None

    @property
    def surround_mode(self) -> SurroundMode | None:
        val = self.state.get(H_SURROUND)
        if val is not None:
            try:
                return SurroundMode(val)
            except ValueError:
                pass
        return None

    @property
    def elevation_mode(self) -> ElevationMode | None:
        val = self.state.get(H_ELEVATION_MODE)
        if val is not None:
            try:
                return ElevationMode(val)
            except ValueError:
                pass
        return None

    @property
    def speaker_color(self) -> SpeakerColor | None:
        val = self.state.get(H_SPEAKER_COLOR)
        if val is not None:
            try:
                return SpeakerColor(val)
            except ValueError:
                pass
        return None

    def add_listener(self, cb: Callable[[], None]) -> Callable[[], None]:
        self._listeners.append(cb)
        return lambda: self._listeners.remove(cb)

    def _fire(self) -> None:
        for cb in list(self._listeners):
            try:
                cb()
            except Exception:
                _LOGGER.exception("Error in listener callback")

    def _note(self, msg: str) -> None:
        self._trace.append(msg)
        _LOGGER.debug("%s: %s", self._name, msg)

    def _trace_str(self) -> str:
        return "; ".join(self._trace)

    def _set_stage(self, stage: str) -> None:
        self._stage = stage
        _LOGGER.debug("%s: %s", self._name, stage)

    # -- connection lifecycle --------------------------------------------------

    async def connect(self) -> None:
        """Connect, pair, and authenticate."""
        async with self._conn_lock:
            if self.connected:
                self._touch()
                return
            await self._drop()
            await self._attempt()
            self._touch()
            self._fire()

    async def _attempt(self, timeout: float = 45.0) -> None:
        self._trace = []
        try:
            async with asyncio.timeout(timeout):
                await self._connect_and_auth()
        except TimeoutError as err:
            raise VizioError(
                f"timed out after {timeout:.0f}s while {self._stage} [{self._trace_str()}]"
            ) from err
        except VizioError as err:
            if self._trace:
                raise VizioError(f"{err} [{self._trace_str()}]") from err
            raise

    async def _establish(self) -> BleakClientWithServiceCache:
        self._set_stage("connecting")
        device = self._get_ble_device()
        if device is None:
            raise VizioError("soundbar is not in Bluetooth range")
        try:
            return await establish_connection(
                BleakClientWithServiceCache,
                device,
                self._name,
                self._on_disconnect,
                max_attempts=3,
                ble_device_callback=self._get_ble_device,
            )
        except Exception as err:
            raise VizioError(f"could not connect: {type(err).__name__}: {err}") from err

    async def _connect_and_auth(self) -> None:
        client = await self._establish()
        self._client = client
        try:
            self._set_stage("pairing")
            try:
                ok = await client.pair()
                self._note(f"pair()={ok}")
            except NotImplementedError:
                pass
            except Exception as err:
                _LOGGER.debug("%s: pair() returned: %s", self._name, err)
            if not client.is_connected:
                await client.disconnect()
                client = self._client = await self._establish()
            self._set_stage("indexing GATT characteristics")
            self._index_characteristics(client)
            await self._authenticate()
            self._set_stage("subscribing to live notifications")
            await self._subscribe_notifiable()
            self._set_stage("reading initial state")
            await self._read_all_state()
        except VizioError:
            raise
        except Exception as err:
            raise VizioError(f"failed while {self._stage}: {type(err).__name__}: {err}") from err

    async def disconnect(self) -> None:
        async with self._conn_lock:
            await self._drop()

    async def close(self) -> None:
        if self._idle_handle:
            self._idle_handle.cancel()
        if self._walker and not self._walker.done():
            self._walker.cancel()
        await self.disconnect()

    async def _drop(self) -> None:
        client, self._client = self._client, None
        self._token = None
        self._subscribed.clear()
        if client is not None:
            try:
                await asyncio.wait_for(client.disconnect(), 10)
            except Exception:
                _LOGGER.debug("error during disconnect", exc_info=True)

    def _on_disconnect(self, _client: Any) -> None:
        self._token = None
        self._subscribed.clear()
        self._fire()

    def _touch(self) -> None:
        if self._idle_handle:
            self._idle_handle.cancel()
            self._idle_handle = None
        if self._idle_timeout and self._client:
            loop = asyncio.get_running_loop()
            self._idle_handle = loop.call_later(self._idle_timeout, self._idle_expired)

    def _idle_expired(self) -> None:
        if self._walker and not self._walker.done():
            self._touch()
            return
        self._idle_task = asyncio.get_running_loop().create_task(self.disconnect())

    # -- GATT plumbing ---------------------------------------------------------

    def _index_characteristics(self, client: BleakClientWithServiceCache) -> None:
        by_attr = {c.handle: c for s in client.services for c in s.characteristics}

        def score(off: int) -> int:
            return sum((h - off) in by_attr for h in KNOWN_HANDLES)

        self._offset = max((0, 1, -1), key=score)
        if score(self._offset) < 5:
            raise VizioError(
                "GATT layout does not match known handles "
                f"({score(self._offset)}/{len(KNOWN_HANDLES)} handles). "
                "Is this the right device, and is it free of other connections?"
            )
        self._chars = {c.handle + self._offset: c for c in by_attr.values()}
        _LOGGER.info(
            "%s: handle offset %+d, %d/%d known handles matched",
            self._name, self._offset, score(self._offset), len(KNOWN_HANDLES),
        )
        self._note(f"offset {self._offset:+d} ({score(self._offset)}/{len(KNOWN_HANDLES)} handles)")

    def _char(self, handle: int) -> BleakGATTCharacteristic:
        try:
            return self._chars[handle]
        except KeyError:
            raise VizioError(f"no characteristic with handle 0x{handle:04x}") from None

    def _on_notify(self, char: BleakGATTCharacteristic, data: bytearray) -> None:
        handle = char.handle + self._offset
        raw = bytes(data)
        self._decode_and_store(handle, raw)
        if self._notifications.qsize() < 100:
            self._notifications.put_nowait((handle, raw))
        self._fire()

    def _decode_and_store(self, handle: int, data: bytes) -> None:
        if not data:
            return

        # Signed 8-bit level characteristics
        if handle in (
            H_LEVEL_BASS,
            H_LEVEL_TREBLE,
            H_LEVEL_CENTER,
            H_LEVEL_SUBWOOFER,
            H_LEVEL_SURROUND,
            H_LEVEL_BALANCE,
        ):
            b = data[0]
            self.state[handle] = b if b < 128 else b - 256
        # Boolean characteristics
        elif handle in (
            H_MUTE,
            H_POWER_LIGHT,
            H_AUTO_HDMI,
            H_VIRTUAL_X,
            H_TRUVOLUME,
            H_NIGHT_MODE,
        ):
            self.state[handle] = (data[0] != 0)
        # Unsigned 8-bit / Enum characteristics
        elif handle in (
            H_VOLUME,
            H_AV_DELAY,
            H_DIALOG,
            H_EQ,
            H_INPUT,
            H_SURROUND,
            H_ELEVATION_MODE,
            H_SPEAKER_COLOR,
            H_STATE_52,
            H_STATE_82,
        ):
            self.state[handle] = data[0]
        # String characteristics
        elif handle in (H_MODEL, H_SERIAL, H_FIRMWARE, H_AUDIO_FORMAT, H_DEVICE_NAME, H_DEVICE_TAG):
            val = data.decode("utf-8", errors="replace").rstrip("\x00").strip()
            self.state[handle] = val
            if handle == H_MODEL:
                self.model = val
            elif handle == H_SERIAL:
                self.serial_number = val
            elif handle == H_FIRMWARE:
                self.firmware_version = val
            elif handle == H_AUDIO_FORMAT:
                self.audio_format = val

    def _drain(self) -> None:
        while not self._notifications.empty():
            self._notifications.get_nowait()

    async def _wait(self, handle: int, timeout: float) -> bytes | None:
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while (left := deadline - loop.time()) > 0:
            try:
                h, v = await asyncio.wait_for(self._notifications.get(), left)
            except TimeoutError:
                return None
            if h == handle:
                return v
        return None

    async def _subscribe(self, handle: int) -> None:
        if handle in self._subscribed:
            return
        assert self._client
        char = self._char(handle)
        if not any(prop in char.properties for prop in ("notify", "indicate")):
            return
        try:
            await asyncio.wait_for(
                self._client.start_notify(char, self._on_notify), 3.0
            )
            self._subscribed.add(handle)
        except Exception as err:
            _LOGGER.debug("%s: start_notify failed for 0x%04x: %s", self._name, handle, err)

    async def _subscribe_notifiable(self) -> None:
        """Subscribe to notifiable characteristics now that authentication is complete."""
        for handle in DEFAULT_SUBSCRIBE_HANDLES:
            if handle in self._chars:
                try:
                    await self._subscribe(handle)
                except Exception as err:
                    _LOGGER.debug("%s: subscribe 0x%04x failed: %s", self._name, handle, err)

    async def _read(self, handle: int) -> bytes:
        assert self._client
        return bytes(
            await asyncio.wait_for(
                self._client.read_gatt_char(self._char(handle)), GATT_OP_TIMEOUT
            )
        )

    async def _write_raw(self, handle: int, data: bytes, response: bool | None = None) -> None:
        assert self._client
        char = self._char(handle)
        if response is None:
            response = "write-without-response" not in char.properties
        await asyncio.wait_for(
            self._client.write_gatt_char(char, data, response=response), GATT_OP_TIMEOUT
        )

    async def _write_cmd(self, handle: int, payload: bytes) -> None:
        if not self._token:
            raise VizioError("not authenticated")
        await self._write_raw(handle, self._token + b":" + payload)

    async def _authenticate(self) -> None:
        self._set_stage("subscribing to auth status")
        try:
            await self._subscribe(H_AUTH_STATUS)
            self._note("sub status ok")
        except Exception as err:
            self._note(f"sub status failed: {type(err).__name__}")
            _LOGGER.warning("%s: status subscribe failed: %s", self._name, err)

        self._set_stage("reading device tag")
        try:
            tag = await self._read(H_DEVICE_TAG)
            self._note(f"tag {tag!r}")
        except Exception as err:
            self._note(f"tag read failed: {type(err).__name__}: {err}")
            _LOGGER.warning("%s: reading device tag failed: %s", self._name, err)

        self._drain()
        self._set_stage("writing client id")
        try:
            await self._write_raw(H_AUTH_CHALLENGE, CLIENT_ID, response=True)
            self._note("client id written (with response)")
        except TimeoutError:
            self._note("client id write timed out; retrying without response")
            await self._write_raw(H_AUTH_CHALLENGE, CLIENT_ID, response=False)
            self._note("client id written (no response)")
        status = await self._wait(H_AUTH_STATUS, 2.0)
        self._note(f"status notify {status.hex() if status is not None else None}")
        if status is None:
            await asyncio.sleep(0.5)

        self._set_stage("reading challenge")
        challenge = await self._read(H_AUTH_CHALLENGE)
        self._note(f"challenge {challenge!r}")

        self._set_stage("writing challenge response")
        await self._write_raw(H_AUTH_RESPONSE, challenge + b":" + AUTH_SUFFIX, response=True)
        status = await self._wait(H_AUTH_STATUS, 2.0)
        self._note(f"status notify {status.hex() if status is not None else None}")
        if status is None:
            await asyncio.sleep(0.5)
        elif status[:1] != b"\x01":
            _LOGGER.warning("unexpected auth status notification: %s", status.hex(" "))

        self._set_stage("reading session token")
        token = await self._read(H_AUTH_RESPONSE)
        if not token:
            raise VizioError("authentication failed (empty session token)")
        self._note("token ok")
        self._token = token

    async def _read_all_state(self) -> None:
        """Read all known readable fields into self.state."""
        for handle in (
            H_MODEL,
            H_SERIAL,
            H_FIRMWARE,
            H_AUDIO_FORMAT,
            H_DEVICE_NAME,
            H_VOLUME,
            H_MUTE,
            H_INPUT,
            H_EQ,
            H_SURROUND,
            H_ELEVATION_MODE,
            H_SPEAKER_COLOR,
            H_POWER_LIGHT,
            H_AUTO_HDMI,
            H_AV_DELAY,
            H_DIALOG,
            H_LEVEL_BASS,
            H_LEVEL_TREBLE,
            H_LEVEL_CENTER,
            H_LEVEL_SUBWOOFER,
            H_LEVEL_SURROUND,
            H_LEVEL_BALANCE,
            H_VIRTUAL_X,
            H_TRUVOLUME,
            H_NIGHT_MODE,
            H_STATE_52,
            H_STATE_82,
        ):
            if handle in self._chars:
                char = self._chars[handle]
                if "read" not in char.properties:
                    continue
                try:
                    data = await asyncio.wait_for(self._read(handle), 1.5)
                    self._decode_and_store(handle, data)
                except Exception as err:
                    _LOGGER.debug("%s: reading 0x%04x failed: %s", self._name, handle, err)

    async def refresh(self) -> None:
        """Connect and refresh state."""
        await self.connect()
        async with self._io_lock:
            await self._read_all_state()
            self._fire()
        self._touch()

    # -- commands --------------------------------------------------------------

    async def _update_byte(self, handle: int, val: int) -> int:
        """Send a single-byte command and await confirmation notification or re-read."""
        await self.connect()
        async with self._io_lock:
            self._drain()
            await self._write_cmd(handle, bytes([val & 0xFF]))
            v = await self._wait(handle, 0.5)
            if v and len(v) == 1:
                self._decode_and_store(handle, v)
                self._fire()
                return v[0]
            # Fallback: re-read
            res = await self._read(handle)
            self._decode_and_store(handle, res)
            self._fire()
            return res[0] if res else val
        self._touch()

    async def set_bool(self, handle: int, val: bool) -> bool:
        res = await self._update_byte(handle, 1 if val else 0)
        return res != 0

    async def set_level(self, handle: int, level: int) -> int:
        """Set a signed -10..+10 level."""
        clamped = max(-10, min(10, int(level)))
        byte_val = clamped & 0xFF
        await self._update_byte(handle, byte_val)
        return clamped

    async def set_bounded_u8(self, handle: int, val: int, max_val: int) -> int:
        """Set an unsigned 0..max_val byte."""
        clamped = max(0, min(max_val, int(val)))
        return await self._update_byte(handle, clamped)

    async def set_mute(self, muted: bool) -> bool:
        return await self.set_bool(H_MUTE, muted)

    async def set_input(self, mode: InputMode | int | str) -> None:
        if isinstance(mode, str):
            resolved = InputMode.from_str(mode)
            if resolved is None:
                raise ValueError(f"Unknown input mode: {mode}")
            mode = resolved
        await self._update_byte(H_INPUT, int(mode))

    async def set_eq(self, mode: EqMode | int | str) -> None:
        if isinstance(mode, str):
            resolved = EqMode.from_str(mode)
            if resolved is None:
                raise ValueError(f"Unknown EQ mode: {mode}")
            mode = resolved
        await self._update_byte(H_EQ, int(mode))

    async def set_surround(self, mode: SurroundMode | int | str) -> None:
        if isinstance(mode, str):
            resolved = SurroundMode.from_str(mode)
            if resolved is None:
                raise ValueError(f"Unknown surround mode: {mode}")
            mode = resolved
        await self._update_byte(H_SURROUND, int(mode))

    async def set_elevation(self, mode: ElevationMode | int | str) -> None:
        if isinstance(mode, str):
            resolved = ElevationMode.from_str(mode)
            if resolved is None:
                raise ValueError(f"Unknown elevation mode: {mode}")
            mode = resolved
        await self._update_byte(H_ELEVATION_MODE, int(mode))

    async def set_speaker_color(self, color: SpeakerColor | int | str) -> None:
        if isinstance(color, str):
            resolved = SpeakerColor.from_str(color)
            if resolved is None:
                raise ValueError(f"Unknown speaker color: {color}")
            color = resolved
        await self._update_byte(H_SPEAKER_COLOR, int(color))

    # -- volume walking --------------------------------------------------------

    async def set_volume(self, level: int) -> None:
        """Request an absolute level. Background task walks there one step at a time."""
        self._target = max(0, min(int(level), self.max_volume))
        if self._walker is None or self._walker.done():
            self._walker = asyncio.get_running_loop().create_task(self._walk())

    async def step_volume(self, delta: int) -> None:
        base = self._target if self._target is not None else self.volume
        if base is None:
            await self.connect()
            base = self.volume or 0
        await self.set_volume(base + delta)

    async def _walk(self) -> None:
        try:
            await self.connect()
            async with self._io_lock:
                cur = (await self._read(H_VOLUME))[0]
                self.volume = cur
                self._fire()
                stalls = 0
                while (target := self._target) is not None and cur != target:
                    nxt = cur + (1 if target > cur else -1)
                    got = await self._write_volume(cur, nxt)
                    if got == cur:
                        stalls += 1
                        if stalls >= 3:
                            raise VizioError(f"volume stuck at {cur} (target {target})")
                    else:
                        stalls = 0
                    cur = got
                self._target = None
        except asyncio.CancelledError:
            raise
        except Exception as err:
            self._target = None
            _LOGGER.warning("volume change failed: %s", err)
        finally:
            self._touch()

    async def _write_volume(self, cur: int, level: int) -> int:
        self._drain()
        await self._write_cmd(H_VOLUME, bytes([level]))
        loop = asyncio.get_running_loop()
        deadline = loop.time() + 1.5
        while (left := deadline - loop.time()) > 0:
            v = await self._wait(H_VOLUME, left)
            if v is None:
                break
            if v and v[0] != cur:
                self.volume = v[0]
                self._fire()
                return v[0]
        res = (await self._read(H_VOLUME))[0]
        self.volume = res
        self._fire()
        return res
