# VIZIO Elevate BLE Home Assistant Integration

A local, push-based Home Assistant custom integration for the **VIZIO Elevate Soundbar** (tested with [VIZIO Elevate SE SL512X-0806](https://www.vizio.com/en/sound-bar/sb/SL512X-0806)) over Bluetooth Low Energy (BLE).

![VIZIO Elevate in Home Assistant](screenshot.png)

---

## Features

* **Media Player (`media_player.py`)**:
  * Volume control with step walking to arbitrary targets
  * Audio mute toggle
  * Input source selection (`Bluetooth`, `HDMI-eARC`)
  * EQ mode selection (`Movie`, `Music`, `Game`, `Direct`)
* **Real-time Audio Format Sensor (`sensor.py`)**:
  * Live decoded format string directly from hardware (e.g. `Dolby Atmos`, `Dolby Audio - Dolby Digital Plus`, `Stereo PCM`)
* **Device Registry Info (`entity.py`)**:
  * Automatically populates hardware model (`0x004a`), serial number (`0x004c`), and firmware version (`0x004e`)
* **Elevation & Audio Mode Selects (`select.py`)**:
  * **Elevation Mode**: `Auto`, `Up`, `Wide` (rotates the physical elevation speakers)
  * **Surround Mode**: `Off`, `On`, `Dual`, `Front`
  * **Speaker LED Color**: `Off`, `Rainbow`, `Light Blue`, `Yellow`, `Purple`, `White`, `Cyan`, `Magenta` *(Configuration)*
* **Audio Processing & Feature Toggles (`switch.py`)**:
  * **DTS Virtual:X** toggle
  * **DTS TruVolume** volume leveling
  * **Night Mode** dynamic range compression
  * **Power Indicator Light** front LED *(Configuration)*
  * **Auto HDMI** input switching *(Configuration)*
* **Level Sliders (`number.py`)**:
  * **Bass** (`-10` to `+10`)
  * **Treble** (`-10` to `+10`)
  * **Center Channel** (`-10` to `+10`)
  * **Subwoofer Level** (`-10` to `+10`)
  * **Surround Level** (`-10` to `+10`)
  * **Surround Balance** (`-10` to `+10`)
  * **Clear Dialog** (`0` to `10`)
  * **AV Lip Sync Delay** (`0` to `8`) *(Configuration)*

---

## Prerequisites (Crucial First Steps)

The soundbar requires BLE Security Manager Protocol (SMP) pairing and link encryption. Because headless Linux/BlueZ does not run an interactive pairing agent for untrusted peripherals, **you must pair and trust the soundbar manually at the OS level before setting it up in Home Assistant.**

### 1. Close Phone Apps
The soundbar only supports **one concurrent BLE client**. Disconnect your phone from the soundbar and force-close the VIZIO mobile app.

### 2. Pair and Trust in `bluetoothctl`
Open a terminal on your Home Assistant host machine (or SSH into HAOS):

```bash
bluetoothctl
```

Inside the `bluetoothctl` shell:

```text
[bluetooth]# scan on
# Wait for "[VIZIO Mobile App]" to appear with its MAC address (e.g. AA:BB:CC:DD:EE:FF)
[bluetooth]# pair AA:BB:CC:DD:EE:FF
[bluetooth]# trust AA:BB:CC:DD:EE:FF
[bluetooth]# quit
```

### 3. Reboot Home Assistant Host
To ensure BlueZ permanently saves the Long Term Key (LTK) and marks the trust flags without any lingering transient D-Bus sessions, **reboot the entire host machine** (e.g. via `sudo reboot` or *Settings -> System -> Hardware -> Reboot Host* in Home Assistant).

---

## Installation

### Via Marketplace (Custom Repository)

> [!NOTE]
>
> At the moment, this custom integration is _not_ discoverable publicly on the HA Marketplace.
>
> I would be more than happy to publish this project for easier discoverability if either one of the following criteria are met:
>
> - A handful of people have installed the extension, confirmed that it works, and/or help catch/fix any low-hanging bugs they find (please leave a comment at [daniel5151/vizio-elevate-ctl#1](https://github.com/daniel5151/vizio-elevate-ctl/issues/1))
> - Someone volunteers to add support for another VIZIO soundbar variant
>     * (the codebase needs some kind of disambiguation infrastructure to support multiple soundbar variants)
>
> Until then, please follow these instructions to install the extension by adding a custom repository.

1. In Home Assistant, open **Marketplace**.
2. Click the three dots (⋮) in the top right corner and select **Custom repositories**.
3. Enter `https://github.com/daniel5151/vizio-elevate-ctl` and select **Integration** as the category.
4. Click **Add**, find **VIZIO Elevate Soundbar (Bluetooth LE)**, and click **Download**.
5. Restart Home Assistant.
6. Add and configure the integration:
   * Go to **Settings** $\rightarrow$ **Devices & Services** $\rightarrow$ **Add Integration** and search for **VIZIO Elevate Soundbar (Bluetooth LE)**:

### Manual Installation
1. Copy the `custom_components/vizio_elevate_ble` directory into your Home Assistant config folder:
   ```text
   /config/custom_components/vizio_elevate_ble/
   ```
2. Restart Home Assistant.
3. Add the integration from **Settings -> Devices & Services -> Add Integration** and search for **VIZIO Elevate Soundbar (Bluetooth LE)**.

---

## Integration Options

Click **Configure** on the VIZIO Elevate card under **Settings -> Devices & Services**:

* **Maximum volume limit (1-100)**:
  Device-level volume guard rail (default: `100`). The Home Assistant volume slider scales to this maximum limit (e.g. setting `40` means `100%` on the slider corresponds to level 40 on the soundbar, and `50%` maps to level 20). The exact soundbar level is also reported in the entity's attributes.
* **Disconnect when idle**:
  * **Off (Default, Recommended)**: Home Assistant keeps the BLE connection open continuously. Volume changes and physical remote presses update instantly with zero latency. *(Locks out the official VIZIO mobile phone app while connected).*
  * **On**: Home Assistant drops the BLE link after 2 minutes of inactivity, freeing the soundbar for the mobile app. Sending commands from Home Assistant will incur a ~1.5s reconnect and authentication handshake.

---

## Standalone Python Diagnostics (`tools/probe.py`)

You can verify the client and read all settings from any machine with Python and Bleak before involving Home Assistant:

```bash
pip install bleak bleak-retry-connector
python tools/probe.py
```

---

## Windows CLI & Protocol Reverse Engineering (`cli/`)

A standalone Windows 10/11 Rust CLI tool for direct control, interactive shell, GATT diffing, and reverse engineering is located in [`cli/`](cli/).

See [`cli/README.md`](cli/README.md) for build instructions, CLI usage, and the reverse-engineered GATT protocol details.

---

## Development Context

One thing that's always bothered me about my VIZIO soundbar is that it doesn't report absolute volume over HDMI-eARC to my TV, making it nigh-impossible to write automations that set the volume to a known-good level (at least without spamming volume down and ramping it back up, hah).

Of course, you can get absolute volume via the VIZIO mobile app...

Fortunately, in 2026, LLMs make it *really easy* to reverse engineer protocols.

After taking a quick `btsnoop` capture of Bluetooth traffic from my Android phone running the VIZIO app, my bot (Gemini 3.8 Flash) and I were able to quickly hack together a utility to pair and control the soundbar's volume from my PC (and shortly thereafter, hack together an MVP of a Home Assistant integration).

...and then, because I couldn't help myself, I ended up spending a few more hours reverse engineering *every other setting* the soundbar exposes.

That includes all sorts of fun things like equalizer settings, effect toggles, and my personal favorite: setting the color of the LEDs on the speakers (which might end up being a great sideband notification signal for other HA automations!)

Based on my internet sleuthing, no one else has written any standalone tools or HA integrations for controlling these VIZIO soundbars over BLE. So, I figured I'd share this work with the community in case anyone else has the same (or similar) soundbar and wants to kick the tires or help extend it.
