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
# Revoke the full keyboard permission: the rule file AND the live ACL on the
# keyboard node. The ACL outlives the rule file — removing only the rule leaves
# a stale grant, so the daemon (which just probes "can I open
# /dev/input/event*?") keeps reading after reinstall and never re-installs the
# rule; the keyboard is then dead after the next reboot. Same one approval.
has_live_acl() {
  local d
  for d in /dev/input/event*; do
    [[ -e "$d" ]] || continue
    udevadm info --query=property --name="$d" 2>/dev/null | grep -qx "ID_INPUT_KEYBOARD=1" || continue
    getfacl -p "$d" 2>/dev/null | grep -q "^user:$(id -un):" && return 0
  done
  return 1
}
if [[ -f "$UDEV_RULE" || -f "$UDEV_RULE_LEGACY" ]] || has_live_acl; then
  if command -v pkexec >/dev/null 2>&1; then
    # Root paths travel as argv ($1/$2), never embedded in shell text.
    # setfacl is scoped to keyboard nodes only — mice/touchpads keep theirs.
    pkexec bash -c 'rm -f "$1" "$2"; udevadm control --reload-rules; udevadm trigger --subsystem-match=input --action=change; for d in /dev/input/event*; do [ -e "$d" ] || continue; if udevadm info --query=property --name="$d" 2>/dev/null | grep -qx "ID_INPUT_KEYBOARD=1"; then setfacl -b "$d" 2>/dev/null || true; fi; done' _ "$UDEV_RULE" "$UDEV_RULE_LEGACY" 2>/dev/null \
      && echo "removed keyboard access (rule + live ACL)" \
      || echo "kept keyboard access (approval declined) — remove with: pkexec rm $UDEV_RULE"
  else
    echo "kept keyboard access (no pkexec) — remove with: sudo rm $UDEV_RULE && sudo setfacl -b /dev/input/event*"
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
