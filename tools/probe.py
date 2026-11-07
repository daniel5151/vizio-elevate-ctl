"""Validate the protocol port on a machine BEFORE involving Home Assistant.

Usage:
    pip install bleak bleak-retry-connector
    python tools/probe.py [name-substring]
"""

from __future__ import annotations

import asyncio
import importlib.util
import logging
from pathlib import Path
import sys

from bleak import BleakScanner

spec = importlib.util.spec_from_file_location(
    "client",
    Path(__file__).parent.parent / "custom_components/vizio_elevate_ble/client.py",
)
assert spec and spec.loader
client_mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client_mod)


async def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s")
    needle = (sys.argv[1] if len(sys.argv) > 1 else "vizio mobile app").lower()

    print(f"Scanning for BLE device matching '{needle}'...")
    dev = await BleakScanner.find_device_by_filter(
        lambda d, ad: needle in (ad.local_name or d.name or "").lower(),
        timeout=15,
    )
    if not dev:
        sys.exit(f"Soundbar matching '{needle}' not found.")

    print(f"Found: {dev.name} ({dev.address})")
    client = client_mod.VizioElevateClient(lambda: dev, idle_timeout=None)

    def on_change():
        print(" -> State notification received!")

    client.add_listener(on_change)

    print("Connecting and authenticating...")
    await client.connect()

    print("\n--- Device Information ---")
    print(f"Model:           {client.model}")
    print(f"Serial:          {client.serial_number}")
    print(f"Firmware:        {client.firmware_version}")
    print(f"Audio Format:    {client.audio_format}")

    print("\n--- Audio & Media Settings ---")
    print(f"Volume:          {client.volume}")
    print(f"Mute:            {client.is_muted}")
    print(f"Input:           {client.input_mode.display_name if client.input_mode is not None else None}")
    print(f"EQ Mode:         {client.eq_mode.display_name if client.eq_mode is not None else None}")
    print(f"Surround Mode:   {client.surround_mode.display_name if client.surround_mode is not None else None}")
    print(f"Elevation Mode:  {client.elevation_mode.display_name if client.elevation_mode is not None else None}")
    print(f"Speaker Color:   {client.speaker_color.display_name if client.speaker_color is not None else None}")

    print("\n--- Audio Levels ---")
    print(f"Bass:            {client.state.get(client_mod.H_LEVEL_BASS)}")
    print(f"Treble:          {client.state.get(client_mod.H_LEVEL_TREBLE)}")
    print(f"Center:          {client.state.get(client_mod.H_LEVEL_CENTER)}")
    print(f"Subwoofer:       {client.state.get(client_mod.H_LEVEL_SUBWOOFER)}")
    print(f"Surround Level:  {client.state.get(client_mod.H_LEVEL_SURROUND)}")
    print(f"Surround Balance:{client.state.get(client_mod.H_LEVEL_BALANCE)}")
    print(f"Clear Dialog:    {client.state.get(client_mod.H_DIALOG)}")
    print(f"AV Delay:        {client.state.get(client_mod.H_AV_DELAY)}")

    print("\n--- Toggles ---")
    print(f"Virtual:X:       {client.state.get(client_mod.H_VIRTUAL_X)}")
    print(f"TruVolume:       {client.state.get(client_mod.H_TRUVOLUME)}")
    print(f"Night Mode:      {client.state.get(client_mod.H_NIGHT_MODE)}")
    print(f"Power Light:     {client.state.get(client_mod.H_POWER_LIGHT)}")
    print(f"Auto HDMI:       {client.state.get(client_mod.H_AUTO_HDMI)}")

    print("\nDisconnecting...")
    await client.close()
    print("Done!")


if __name__ == "__main__":
    asyncio.run(main())
