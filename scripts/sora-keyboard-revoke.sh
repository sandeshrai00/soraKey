#!/bin/bash
# sora-keyboard-revoke.sh — manual keyboard-permission revoke. Removal (panel
# Uninstall or menu delete) deliberately KEEPS the permission: the udev rule
# in /etc plus the live ACL survive, so a reinstall or reboot re-asks
# nothing. Run this in a terminal whenever you want the permission itself
# gone (one password, yours to type):
#   sudo ~/.local/lib/sorakey/sora-keyboard-revoke.sh
# Staged at install time to ~/.local/lib/sorakey/ so it outlives the plugin.
set -euo pipefail
: "${HOME:?HOME is unset, refusing to revoke}"
UDEV_RULE="/etc/udev/rules.d/70-sora-keyboard.rules"
UDEV_RULE_LEGACY="/etc/udev/rules.d/70-sorakey-keyboard.rules"
SHARE="$HOME/.local/share/sorakey"

notify() {
  # best effort — the session bus may be gone (shutdown race); never fail on it
  if command -v omarchy-notification-send >/dev/null 2>&1; then
    omarchy-notification-send --app-name Sorakey -u normal "$1" "$2" 2>/dev/null || true
  fi
}

# Full data wipe first (user files, no approval needed).
rm -rf "$SHARE" "$HOME"/.local/share/sorakey.bak.* "$HOME/.cache/sorakey" 2>/dev/null || true
rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sorakey.sock" "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sorakey.lock" "$HOME/.sorakey.sock" "$HOME/.sorakey.lock" 2>/dev/null || true

# Rule + live ACL (needs one approval). As root (sudo) run directly — no
# second dialog. Otherwise pkexec pops the GUI approval.
revoke_as_root() {
  local rule="$1" legacy="$2"
  rm -f "$rule" "$legacy"
  udevadm control --reload-rules
  udevadm trigger --subsystem-match=input --action=change
  local d
  for d in /dev/input/event*; do
    [ -e "$d" ] || continue
    # setfacl stays scoped to keyboard nodes only — mice/touchpads keep theirs
    if udevadm info --query=property --name="$d" 2>/dev/null | grep -qx "ID_INPUT_KEYBOARD=1"; then
      setfacl -b "$d" 2>/dev/null || true
    fi
  done
}

revoked=0
if [[ -f "$UDEV_RULE" || -f "$UDEV_RULE_LEGACY" ]]; then
  if [[ "$(id -u)" == "0" ]]; then
    revoke_as_root "$UDEV_RULE" "$UDEV_RULE_LEGACY" && revoked=1 || true
  elif command -v pkexec >/dev/null 2>&1; then
    # Function body travels via declare -f (never embedded in shell text);
    # paths travel as argv ($1/$2), never embedded either.
    pkexec bash -c 'eval "$1"; revoke_as_root "$2" "$3"' _ "$(declare -f revoke_as_root)" "$UDEV_RULE" "$UDEV_RULE_LEGACY" 2>/dev/null && revoked=1 || true
  fi
  if [[ "$revoked" == 0 ]]; then
    notify "Sorakey: keyboard permission kept" "Approval declined — revoke it with: sudo ~/.local/lib/sorakey/sora-keyboard-revoke.sh"
  else
    echo "revoked keyboard permission (rule + live ACL)"
  fi
else
  echo "no keyboard permission installed — nothing to revoke"
fi
