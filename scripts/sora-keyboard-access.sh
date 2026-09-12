#!/bin/bash
# Sorakey keyboard-access enabler — run from the panel's "Enable keyboard
# sounds" button. One GUI approval (via the shell's polkit agent), no
# terminal commands, no logout.
#
# Installs udev/70-sora-keyboard.rules to /etc/udev/rules.d/ (TAG+=uaccess
# for ID_INPUT_KEYBOARD devices), reloads rules and triggers them, then
# verifies the current user can read a keyboard event node.
#
# Exit codes: 0 = access works, 1 = hard error, 2 = approval not granted
# (panel stays truthful and offers Retry), 3 = no approval dialog exists
# on this box (panel offers the terminal route instead).
#
# Flags: --use-sudo  skip pkexec and use sudo directly (for terminal use,
# where a TTY exists for the password prompt).
set -euo pipefail

USE_SUDO=0
if [[ "${1:-}" == "--use-sudo" ]]; then USE_SUDO=1; fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_DIR="$(dirname "$SCRIPT_DIR")"
SRC="$PLUGIN_DIR/udev/70-sora-keyboard.rules"
DST="/etc/udev/rules.d/70-sora-keyboard.rules"
CONSENT_FILE="$HOME/.local/share/sorakey/keyboard-granted"

step() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
note() { printf '%s\n' "$*"; }

# Consent note: playback additionally requires this file (uid + timestamp).
# Removal deletes it, but the OS grant survives by design — so after a
# reinstall the early exit below re-writes it silently (no dialog) whenever
# the keyboard is already readable. Never fails the script (best effort).
write_consent() {
  mkdir -p "$(dirname "$CONSENT_FILE")" 2>/dev/null || true
  printf '%s %s\n' "$(id -u)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$CONSENT_FILE" 2>/dev/null || true
}

[[ -f "$SRC" ]] || { echo "sora-keyboard-access: rule source missing: $SRC" >&2; exit 1; }

# A keyboard event node the current user should be able to read.
find_keyboard_node() {
  local d
  for d in /dev/input/event*; do
    [[ -e "$d" ]] || continue
    if udevadm info --query=property --name="$d" 2>/dev/null | grep -qx "ID_INPUT_KEYBOARD=1"; then
      printf '%s' "$d"
      return 0
    fi
  done
  return 1
}

user_can_read_keyboard() {
  local node
  node=$(find_keyboard_node) || return 1
  [[ -r "$node" ]]
}

run_privileged() {
  # $1 = shell snippet needing root. pkexec pops the shell's GUI dialog;
  # --use-sudo takes sudo (terminal provides the password prompt).
  # Prints the helper's stderr to $PK_ERR_FILE when set, for exit mapping.
  # Injection-proof: SRC/DST travel as positional parameters ($1/$2), never
  # embedded in shell text — quotes or spaces in the path can't escape.
  local root_cmd='install -m 644 "$1" "$2" && udevadm control --reload-rules && udevadm trigger --subsystem-match=input --action=change'
  local out=""
  if [[ "$USE_SUDO" -eq 0 ]] && ! command -v pkexec >/dev/null 2>&1 && ! command -v sudo >/dev/null 2>&1; then
    # Neither approval tool exists: distinct from a user dismissal, or the
    # panel would offer Retry in an infinite loop.
    printf '%s' "no privilege tool (pkexec/sudo) available" > "${PK_ERR_FILE:-/dev/null}" 2>/dev/null || true
    return 1
  fi
  if [[ "$USE_SUDO" -eq 0 ]] && command -v pkexec >/dev/null 2>&1; then
    out=$(pkexec bash -c "$root_cmd" _ "$SRC" "$DST" 2>&1) && return 0
    printf '%s' "$out" > "${PK_ERR_FILE:-/dev/null}" 2>/dev/null || true
    note "(not approved)"
  elif command -v sudo >/dev/null 2>&1; then
    out=$(sudo bash -c "$root_cmd" _ "$SRC" "$DST" 2>&1) && return 0
    printf '%s' "$out" > "${PK_ERR_FILE:-/dev/null}" 2>/dev/null || true
    # terminal mode: the user is watching, so show the failure live too
    if [[ "$USE_SUDO" -eq 1 && -n "$out" ]]; then printf '%s\n' "$out" >&2; fi
  fi
  return 1
}

# Map a failed privilege attempt to an exit code from the helper's stderr:
# user dismissal -> 2, missing dialog/session plumbing -> 3, else 1.
map_priv_error() {
  local err="$1" low=""
  low=$(printf '%s' "$err" | tr '[:upper:]' '[:lower:]')
  case "$low" in
    *dismiss*|*cancel*) return 2 ;;
    *privilege?tool*|*agent*|*authority*|*session*|*polkit*|*display*|*terminal*required*) return 3 ;;
    *) return 1 ;;
  esac
}

step "Keyboard sounds"
if user_can_read_keyboard; then
  note "Ready. Type to hear sounds."
  if [[ -f "$DST" ]] && ! cmp -s "$SRC" "$DST"; then
    note "Installed permission is outdated — it refreshes on next approval."
  fi
  write_consent
  exit 0
fi
note "Keyboard permission needed for sounds."

step "Approving (one time)"
PK_ERR_FILE=$(mktemp)
trap 'rm -f "$PK_ERR_FILE"' EXIT
if ! run_privileged; then
  err=$(cat "$PK_ERR_FILE" 2>/dev/null)
  rm -f "$PK_ERR_FILE"
  trap - EXIT
  code=2
  if [[ -n "$err" ]]; then
    # map_priv_error returns nonzero by design (it IS the code); suspend
    # errexit around it or set -e aborts before the assignment.
    set +e
    map_priv_error "$err"
    code=$?
    set -e
  fi
  if [[ "$code" -eq 3 ]]; then
    echo "sora-keyboard-access: no approval dialog on this system" >&2
    exit 3
  fi
  if [[ "$code" -eq 1 ]]; then
    printf '%s\n' "$err" >&2
    exit 1
  fi
  exit 2
fi
rm -f "$PK_ERR_FILE"

# udev applies access shortly — check briefly as the user.
step "Checking"
for ((i = 0; i < 10; i++)); do
  if user_can_read_keyboard; then
    note "Done. Type to hear sounds."
    write_consent
    exit 0
  fi
  sleep 1
done

echo "sora-keyboard-access: approved, but keys are still unreadable" >&2
exit 1
