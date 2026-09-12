# Sorakey Cleanup Checklist

Run this command to check if all Sorakey files are removed:

```bash
find ~/.config ~/.local ~/.cache -name "*sorakey*" -o -name "*Sorakey*" -o -name "*SORAKEY*" 2>/dev/null
```

## Files/Locations to Check

| Path | Description |
|------|-------------|
| `~/.config/omarchy/plugins/*sorakey*` | Plugin files |
| `~/.local/bin/sorakey` | Binary |
| `~/.local/share/sorakey/` | Soundpacks |
| `~/.local/share/sorakey.bak.*` | Old backups (`--purge` removes) |
| `~/.local/lib/sorakey/` | Build cache |
| `~/.cache/sorakey/` | Temp cache |
| `~/.config/sorakey/` | Panel prefs (logo, corners) |
| `~/.config/systemd/user/sorakey.service` | Systemd service |
| `/etc/udev/rules.d/70-sora-keyboard.rules` | Keyboard-access rule + live ACL (both revoked with one approval) |
| `$XDG_RUNTIME_DIR/sorakey.{sock,lock}` | Daemon runtime files |

The panel's Uninstall button runs `sora-uninstall.sh --purge` (full wipe above)
plus `omarchy plugin remove`. Removed via Omarchy's menu instead? Same
standard: the daemon stops itself instantly and a helper wipes everything
above (including the rule row) with one approval dialog — decline it and use
the one-liner from `docs/keyboard-access.md`. Exported log files (your chosen location)
are yours and are never deleted.

## Services to Check

```bash
systemctl --user list-units --all | grep sorakey
systemctl --user list-unit-files | grep sorakey
```

If any output appears, run:
```bash
systemctl --user disable --now sorakey
systemctl --user daemon-reload
```
