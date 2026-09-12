#!/bin/bash
# sora-keyboard-revoke.sh — menu-deletion cleanup, launched by the daemon's
# orphan self-clean (fully detached) after Omarchy deletes the plugin folder.
# The plugin dir (where sora-uninstall.sh lives) is already gone then, so
# this helper is staged at install time to ~/.local/lib/sorakey/ and runs
# from there. Same standard as the panel's Uninstall button: full data wipe
# (no .bak) + rule/ACL revoke with one approval (pkexec).
#
# Usage: sora-keyboard-revoke.sh [--self-clean]
# --self-clean deletes the helper's own directory at the end (only the
# daemon passes it; running the script by hand without the flag is safe).
set -euo pipefail
: "${HOME:?HOME is unset, refusing to revoke}"
SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UDEV_RULE="/etc/udev/rules.d/70-sora-keyboard.rules"
UDEV_RULE_LEGACY="/etc/udev/rules.d/70-sorakey-keyboard.rules"
SHARE="$HOME/.local/share/sorakey"

notify() {
  # best effort — the session bus may be gone (shutdown race); never fail on it
  if command -v omarchy-notification-send >/dev/null 2>&1; then
    omarchy-notification-send --app-name Sorakey -u normal "$1" "$2" 2>/dev/null || true
  fi
}

# 1. Full data wipe first (user files, no approval needed). Even if the
# pkexec below is declined, menu removal leaves no data behind.
rm -rf "$SHARE" "$HOME"/.local/share/sorakey.bak.* "$HOME/.cache/sorakey" 2>/dev/null || true
rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sorakey.sock" "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sorakey.lock" "$HOME/.sorakey.sock" "$HOME/.sorakey.lock" 2>/dev/null || true

# 2. Rule + live ACL (needs one approval). Same argv-passing snippet as
# sora-uninstall.sh; setfacl stays scoped to keyboard nodes only.
revoked=0
if [[ -f "$UDEV_RULE" || -f "$UDEV_RULE_LEGACY" ]]; then
  if command -v pkexec >/dev/null 2>&1; then
    pkexec bash -c 'rm -f "$1" "$2"; udevadm control --reload-rules; udevadm trigger --subsystem-match=input --action=change; for d in /dev/input/event*; do [ -e "$d" ] || continue; if udevadm info --query=property --name="$d" 2>/dev/null | grep -qx "ID_INPUT_KEYBOARD=1"; then setfacl -b "$d" 2>/dev/null || true; fi; done' _ "$UDEV_RULE" "$UDEV_RULE_LEGACY" 2>/dev/null && revoked=1 || true
  fi
  if [[ "$revoked" == 0 ]]; then
    notify "Sorakey: keyboard permission kept" "Approval declined — revoke it with: pkexec rm $UDEV_RULE"
  fi
fi

# 3. Self-delete: nothing of the plugin may survive menu removal.
if [[ "${1:-}" == "--self-clean" ]]; then
  rm -rf "$SELF_DIR"
fi
