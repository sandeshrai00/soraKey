#!/bin/bash
set -euo pipefail
# Empty HOME would turn every path below into a system-absolute one —
# refuse instead of rm -rf'ing the wrong tree.
: "${HOME:?HOME is unset, refusing to uninstall}"
PURGE=0
if [[ "${1:-}" == "--purge" ]]; then PURGE=1; fi
UDEV_RULE="/etc/udev/rules.d/70-sora-keyboard.rules"
UDEV_RULE_LEGACY="/etc/udev/rules.d/70-sorakey-keyboard.rules"
echo "== Sorakey uninstall =="
systemctl --user disable --now sorakey 2>/dev/null || true
systemctl --user daemon-reload 2>/dev/null || true
pkill -x sorakey 2>/dev/null || true
# remove the keyboard-access rule installed via the panel (GUI-approved);
# needs one approval, like the install did
if [[ -f "$UDEV_RULE" || -f "$UDEV_RULE_LEGACY" ]]; then
  if command -v pkexec >/dev/null 2>&1; then
    pkexec bash -c "rm -f '$UDEV_RULE' '$UDEV_RULE_LEGACY' && udevadm control --reload-rules && udevadm trigger --subsystem-match=input --action=change" 2>/dev/null \
      && echo "removed keyboard-access rule" \
      || echo "kept $UDEV_RULE (approval declined) — remove with: pkexec rm $UDEV_RULE"
  else
    echo "kept $UDEV_RULE (no pkexec) — remove with: sudo rm $UDEV_RULE"
  fi
fi
if [[ $PURGE -eq 1 ]]; then
  rm -rf "$HOME/.local/share/sorakey" "$HOME"/.local/share/sorakey.bak.* "$HOME/.local/bin/sorakey" "$HOME/.config/systemd/user/sorakey.service" "$HOME/.cache/sorakey" "$HOME/.local/lib/sorakey" "$HOME/.config/sorakey"
  rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sorakey.sock" "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sorakey.lock" "$HOME/.sorakey.sock" "$HOME/.sorakey.lock"
  echo "purged data + binary + prefs + runtime files"
else
  # keep packs as .bak
  if [[ -d "$HOME/.local/share/sorakey" ]]; then
    bak="$HOME/.local/share/sorakey.bak.$(date +%s)"
    if mv "$HOME/.local/share/sorakey" "$bak" 2>/dev/null; then
      echo "moved packs to $bak (use --purge to delete)"
    else
      echo "WARNING: could not move packs aside; they remain at $HOME/.local/share/sorakey" >&2
    fi
  fi
  rm -f "$HOME/.local/bin/sorakey" "$HOME/.config/systemd/user/sorakey.service"
  systemctl --user daemon-reload 2>/dev/null || true
fi
# remove plugin with: omarchy plugin remove io.github.sandeshrai00.sorakey --yes
echo "run: omarchy plugin remove io.github.sandeshrai00.sorakey --yes ; omarchy restart shell"
