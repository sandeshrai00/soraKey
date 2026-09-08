# Sorakey

Mechanical keyboard sounds for Omarchy, driven by a lean Rust
daemon. A keyboard icon on the bar opens a panel with live mute, volume,
and soundpack picker; right-click the icon to mute, scroll to
set volume.

## What it is

- **`sorakey`** — a headless sound daemon forked from the [MechvibesDX](https://github.com/hainguyents13/mechvibes-dx)
  v0.8.2 audio core: same polyphonic engine, anti-click fades, resampler and
  V2 soundpack format, with all of the GUI, tray, telemetry and auto-updater
  removed. It runs as a `systemd` user service, idles at ~0% CPU and ~40 MB RAM,
  and is controlled over a Unix socket.
- **The Omarchy plugin** — a bar widget + panel that installs the daemon
  (one click) and controls it live.

## Install

```sh
omarchy plugin add https://github.com/sandeshrai00/soraKey.git --enable
```

1. Open the Sorakey panel on your bar and click **Install Sorakey**. It installs
   `sorakey` and starts the service. The installer prefers the **prebuilt
   binary from GitHub Releases** (a few seconds): it verifies the release
   checksum, and when `gh` is logged in (or `GH_TOKEN` is set) also the GitHub
   attestation proving CI built it from the tagged commit. Without a `gh`
   login the checksum check alone is accepted. It only builds from source
   (needs Rust, a few minutes) when no release matches the current daemon
   source — e.g. you changed `daemon/` but haven't tagged a new version yet.
2. For **keyboard** sounds the panel shows a **keyboard-access step**.
   Tap **Enable keyboard sounds** and approve the one-time system dialog
   (password or fingerprint). This installs one rule for **keyboards only**
    (`/etc/udev/rules.d/70-sora-keyboard.rules`) — no logout, no restart,
   no terminal. Sounds start within seconds. Why this is needed and how
   removal works: `docs/keyboard-access.md`.

## Usage

- **Left-click** the keyboard icon → panel: mute switch, per-pack volume
  slider, soundpack dropdown (search, delete), **Random**, **Import Sound**,
  **Open Folder**, a live **Test typing** box, and Start/Stop/Restart.
- **Right-click** → toggle mute. **Scroll** on the icon → volume.
- **Ctrl+Alt+M** → global mute, from anywhere (system-wide hotkey).
- **Escape** closes the panel, **Tab** / **Shift+Tab** switches panels.
- **Settings** (gear in the panel header): bar icon position, audio output
  device, and **Export error logs** (saves a report of recent errors to a
  file via a GTK save dialog, or `~/Downloads` when run from a terminal).

The volume slider is **per soundpack**: each pack keeps its own level, and
clicking the percentage label resets it to that pack's recommended default.
Packs whose `config.json` sets `options.recommended_volume` start at that
level automatically.

## Output device

Settings → **AUDIO OUTPUT** lists the system's output devices (plus
**System default**). Pick one to route keyboard sounds there; **Rescan
devices** refreshes the list. The choice persists and is reported by
`status` as `audio_device`.

**System default follows your OS**: if the default sink changes (e.g. you
plug in a headset that becomes the default), the daemon reopens its stream
on the new sink within seconds — no restart needed. An explicitly picked
device is retried for ~60s when missing at startup (headsets that appear
late after boot), then falls back to the default and says so. `status`
also reports `audio_device_opened` — what the live stream is actually on —
so the shown selection can never silently disagree with reality.
Unplugging the selected device still goes quiet by design; reselect in
Settings.

## Configure

```sh
omarchy bar move io.github.sandeshrai00.sorakey --section center
```

Mute, volume and soundpack persist across restarts
(`~/.local/share/sorakey/data/config.json`); the icon section persists across
disable/re-enable.

## Auto-start

The daemon is a `systemd` user service, enabled at install, so it starts
automatically at login. The panel's **Stop** halts it for the current session
(without disabling it) — it comes back at the next login. **Start** brings it
back immediately. To turn auto-start off entirely, disable the unit
(`systemctl --user disable sorakey`) or remove the plugin (see Remove).

## Update

```sh
omarchy plugin update io.github.sandeshrai00.sorakey --yes
```

The QML updates in place. If the daemon source changed, the next shell start
(or plugin reload) re-runs the installer — it downloads the CI prebuilt for
that exact source (matched by content hash, never by tag) and restarts the
daemon automatically. There is no source build on your machine: if no
prebuilt matches yet (fresh commit, CI still building ~10 min), the panel
shows the reason and keeps the current binary. Bundled soundpacks re-sync
the same way (changed packs refresh, removed packs disappear, your imported
packs are never touched) and the panel shows a "Soundpacks updated" toast
when it happens.

## Remove

```sh
# stops the daemon, removes binary + service file, keeps soundpacks as .bak
~/.config/omarchy/plugins/io.github.sandeshrai00.sorakey/scripts/sora-uninstall.sh
omarchy plugin remove io.github.sandeshrai00.sorakey --yes
omarchy restart shell
```

> Panel path differs: the panel's Uninstall button runs
> `sora-uninstall.sh --purge` — a FULL wipe (binary, service, packs,
> settings, caches). The manual commands above keep your packs as a
> timestamped `.bak` instead; pass `--purge` yourself for the same wipe.

`sora-uninstall.sh` moves `~/.local/share/sorakey` (packs **and** settings) to a
`.bak` timestamped folder; `sora-uninstall.sh --purge` deletes it instead. The
final `omarchy restart shell` drops the bar icon.

## Import a soundpack

Click the keyboard icon on the bar → **Import Sound** → pick a `.zip`.
The pack is extracted to `~/.local/share/sorakey/soundpacks/keyboard/{id}/`
and appears in the keyboard dropdown on the next refresh. The ZIP must
contain a `config.json` (V2 format).

## Control API

`sorakey ctl '<json>'` speaks one JSON line in, one JSON line out, over
`$XDG_RUNTIME_DIR/sorakey.sock`:

- `status` — running, muted, volume, per-pack volume, active pack, audio device
- `mute {"muted": true|false}`
- `volume {"value": 0-100}` — global level (used when no pack is selected)
- `per_pack_volume {"id": "keyboard/...", "value": 0-100}`
- `reset_volume {"id": "keyboard/..."}` — back to the pack's recommended level
- `keyboard_pack {"id": "keyboard/..."}` — switch the active pack
- `packs` — list available packs
- `delete_pack {"id": "keyboard/..."}` — remove a pack (falls back to another)
- `audio_devices` — list output devices + current selection
- `select_device {"id": "..."}` — route output there (`null` = system default)
- `set_bar_section {"section": "left|center|right"}` / `get_bar_section`
- `diag` — memory (RSS/HWM), per-pack entry + cache sizes, active pack
- `export_logs` — recent error log as text, with a suggested filename

## Diagnostics & logs

- **Memory / cache**: `sorakey ctl '{"cmd":"diag"}'` reports resident
  memory, the soundpack cache size, and per-pack volume entries — useful for
  spotting unbounded growth.
- **Error logs**: the daemon keeps a rolling buffer of recent errors.
  Settings → **Export error logs** opens a GTK save dialog (or run
  `scripts/sora-export-logs.py <name>` from a terminal to write straight to
  `~/Downloads`). The file is named `sorakey-log-<timestamp>.txt`.

## Files

| Path | What |
|---|---|
| `manifest.json` | Omarchy plugin manifest (bar-widget + service) |
| `SoraWidget.qml` | Bar icon + popup panel (with **Import Sound** button) |
| `SoraService.qml` | Headless service (daemon lifecycle + import flow) |
| `SoraKeyStore.js` | Status/pack parsing helpers |
| `SoraPackPicker.qml` | Searchable soundpack picker |
| `scripts/sora-install` | One-click installer |
| `scripts/sora-build.sh` | Prebuilt-only daemon install (content-hash matched) + bundled-pack sync |
| `scripts/sora-keyboard-access.sh` | One-approval keyboard-access enabler (udev rule) |
| `scripts/sorakey-detached` | Reload-proof detached helper launcher (import/export dialogs) |
| `scripts/sora-pack-import.py` | GTK4 file-picker + ZIP extractor |
| `scripts/sora-export-logs.py` | Log export (dialog or `~/Downloads`) |
| `scripts/_v1_shared.py` | Shared V1 tables + result-file channel for the pickers |
| `scripts/sora-uninstall.sh` | Removes daemon, unit file, binary (`--purge`: all data too) |
| `udev/70-sora-keyboard.rules` | Keyboard-access udev rule source |
| `daemon/` | The `sorakey` Rust daemon (trimmed MechvibesDX core) |
| `daemon/soundpacks/` | Built-in V2 soundpacks |
| `daemon/` | The `sorakey` Rust daemon (trimmed MechvibesDX core) |
| `daemon/soundpacks/` | Built-in V2 soundpacks |

## Layout

```
~/.local/bin/sorakey                 binary
~/.local/share/sorakey/soundpacks/    built-in + imported packs
~/.local/share/sorakey/data/config.json   settings (persisted by ctl)
~/.local/share/sorakey/bar-section    last bar section chosen in Settings
$XDG_RUNTIME_DIR/sorakey.sock         control socket
```

## Build the daemon by hand (developers only)

```sh
SORAKEY_ALLOW_SOURCE=1 ./scripts/sora-build.sh
# — or directly:
cargo build --release --manifest-path daemon/Cargo.toml
```

Requires `rustc` plus the native libs `alsa`, `libevdev`, `libx11`,
`pkg-config` (already present on an Omarchy box). User machines never do
this: installs use the CI prebuilt or refuse loudly.

## License

MIT. The daemon is a fork of the MIT-licensed MechvibesDX core
(Copyright (c) 2026 Hải Nguyễn); see `LICENSE`.