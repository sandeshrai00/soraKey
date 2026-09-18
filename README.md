# Sorakey

Mechanical keyboard sounds for the Omarchy Quattro bar — lean Rust daemon + bar widget with live mute, volume and soundpacks.

## Install

```sh
omarchy plugin add https://github.com/sandeshrai00/soraKey.git --enable
```

1. Open the Sorakey panel on your bar and click **Install Sorakey**.
2. Tap **Enable keyboard sounds** and approve the one-time system dialog (keyboards only, no logout).

Sounds start within seconds. See `docs/keyboard-access.md` for permission details.

## Usage

- **Left-click** the icon → panel with mute, per-pack volume, soundpack picker (search, **Random**, **Import Sound**, **Open Folder**), live **Test typing**, and Start/Stop/Restart
- **Right-click** → toggle mute · **Scroll** on the icon → adjust volume · **Ctrl+Alt+M** → global mute
- **Settings** (gear) → move the bar icon, choose audio output device, and **Export error logs**

## Update

Panel → **Check for Update** → **Update Sorakey**, or:

```sh
omarchy plugin update io.github.sandeshrai00.sorakey --yes
omarchy restart shell
```

## Remove

```sh
~/.config/omarchy/plugins/io.github.sandeshrai00.sorakey/scripts/sora-uninstall.sh
omarchy plugin remove io.github.sandeshrai00.sorakey --yes
omarchy restart shell
```

Keyboard permission (`/etc/udev/rules.d/70-sora-keyboard.rules`) is kept by design — revoke anytime with `sudo ~/.local/lib/sorakey/sora-keyboard-revoke.sh` (see `docs/keyboard-access.md`).

## Acknowledgements

Daemon audio core is derived from [MechvibesDX](https://github.com/hainguyents13/mechvibes-dx) by Hai Nguyen. Forked as a headless daemon with anti-click fades, resampling and V2 soundpack format — GUI, tray, telemetry and auto-updater removed.

## License

MIT — see [LICENSE](LICENSE).

Copyright (c) 2026 sandeshrai00.
