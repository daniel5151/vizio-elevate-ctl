# vizio-ctl

Control a VIZIO Elevate soundbar over Bluetooth LE from Windows 10/11.

Only tested with VIZIO Elevate SE (Model: SL512X-0806).

## Build

```powershell
cargo build --release
# binary: target\release\vizio-ctl.exe
```

## Usage

> Close the VIZIO app / turn off Bluetooth on your phone first: the soundbar
> typically only accepts one BLE connection at a time.

```powershell
vizio-ctl scan                 # list paired + nearby BLE devices (looks for "VIZIO")
vizio-ctl pair                 # optional OS-level pairing
vizio-ctl info                 # connect, authenticate, print device info/state
vizio-ctl volume               # print volume
vizio-ctl volume set 25
vizio-ctl volume up 3
vizio-ctl volume down
vizio-ctl mute                 # toggle mute (or on, off, get)
vizio-ctl shell                # interactive; keeps connection open (+, -, m, diff, watch, get, q)
vizio-ctl dump                 # every service/characteristic with ATT handles + values
vizio-ctl monitor              # print all notifications
vizio-ctl read 0x0076          # raw read
vizio-ctl write 0x006a 01      # raw write (session token prefix added automatically)
```

Global flags: `--address AA:BB:CC:DD:EE:FF`, `--name VIZIO`, `--scan-secs 10`, `-v` (log all GATT traffic).
The address is remembered in `vizio-ctl.toml` next to the executable after the first successful connection.

## Discovering Features via Remote

To reverse-engineer unknown settings (EQ modes, inputs, bass/treble, surround modes, etc.) using your physical remote:

1. Launch the interactive shell:
   ```powershell
   vizio-ctl shell
   ```
2. **Diffing state (`diff` / `d`)**:
   - Run `diff` (or `d`) to establish a baseline snapshot of known fields:
     ```text
     vizio> diff
     Baseline snapshot saved (13 fields). Press a button on the remote, then type 'diff'.
     ```
   - Press a button on the remote (e.g. EQ mode).
   - Run `diff` again to print only the fields that changed:
     ```text
     vizio> diff
     Changes detected:
       0x0076 (0x0076 (unknown, maybe EQ mode)): 3 (0x03) -> 1 (0x01)
     ```
   - **`diff all` / `d all`**: Snapshot and diff **all ~50 readable GATT characteristics** across the device. Use this if a button press didn't show up in `diff` (meaning it's not in the known fields list yet).
   - **`snap`**: Re-takes the baseline snapshot at any time.

3. **Live notification watching (`watch`)**:
   - Run `watch` to subscribe to all notification-capable characteristics:
     ```text
     vizio> watch
     Subscribed to 8 characteristics. Notifications will print live at prompt.
     ```
   - When the soundbar emits a notification in response to remote presses or internal events, it prints directly above the prompt:
     ```text
     [notify 0x0076 -> 01]
     vizio>
     ```

4. **Testing writes**:
   - Verify control by sending raw payloads with `write <handle> <hex>` (session token prefix is attached automatically):
     ```text
     vizio> write 0x006a 01    # mute
     vizio> write 0x006a 00    # unmute
     ```

## Protocol (from the btsnoop capture)

| Step | Op        | Handle | Data                                                                              |
| ---- | --------- | ------ | --------------------------------------------------------------------------------- |
| 1    | read      | 0x0056 | `"86EB"` (device tag)                                                             |
| 2    | write req | 0x00c2 | `"1:f4c46a1e-4e22-"` → notify 0x00bf = `00`                                       |
| 3    | read      | 0x00c2 | 4-char challenge, e.g. `"8C24"`                                                   |
| 4    | write req | 0x00c5 | `"<challenge>:" f8 b8 18 40 c8 20 98 e0 08 08 20 90 38 00` → notify 0x00bf = `01` |
| 5    | read      | 0x00c5 | 8-char session token, e.g. `"371FD047"`                                           |
| vol  | write cmd | 0x0079 | `"<token>:" <level>` → notify 0x0079 = `<level>`                                  |
| mute | write cmd | 0x006a | `"<token>:" 01` (mute) / `00` (unmute)                                            |

The 14 trailing auth bytes were identical in two sessions with different challenges, so they're replayed as-is.
Volume values are absolute levels (the app sends 0x15, 0x16, 0x17... for each "+" press), but the soundbar
only moves **one step per write** toward the requested value (writing 11 at volume 8 yields 9). `vizio-ctl`
therefore walks to the target one level at a time, waiting for each 0x0079 confirmation notification.
Use `--max-volume N` to cap how high it will go.
