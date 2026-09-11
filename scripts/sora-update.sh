#!/bin/bash
# sora-update.sh — Sorakey self-update (panel Update button).
# Must run double-forked via sorakey-detached: a direct child of the panel's
# Process would be SIGTERMed by the reload its own merge triggers.
# $1 = optional result file (sorakey-detached contract: append one line).
#
# Single brain for applying: run the official `omarchy plugin update`
# (fetch + fast-forward merge + validate + auto-rollback), then decide —
# HEAD ahead of the version stamp means the running shell is stale (a fresh
# merge OR a terminal update already on disk), so restart; otherwise report
# up to date. The running shell's hot reload never replaces the live
# bar-widget instance, so the restart IS the apply step.
# SORAKEY_UPDATE_NO_RESTART=1 skips the actual restart (test seam): stamp,
# marker and result still happen so tests can verify the decision.
set -uo pipefail
result="${1:-}"
id="io.github.sandeshrai00.sorakey"
plugin_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data_dir="$HOME/.local/share/sorakey"
stamp="$data_dir/session-head"
notice="$data_dir/pending-update"

notify() { omarchy-notification-send --app-name Sorakey "$1" "$2" 2>/dev/null || true; }
restart_shell() {
  [[ "${SORAKEY_UPDATE_NO_RESTART:-}" == "1" ]] && return 0
  setsid omarchy restart shell >/dev/null 2>&1 &
}

rc=0
out=$(omarchy plugin update "$id" --yes 2>&1) || rc=$?
last=$(printf '%s\n' "$out" | tail -n 1)
[[ -n $out ]] && printf '%s\n' "${out//$id/Sorakey}"

head=$(git -C "$plugin_dir" rev-parse HEAD 2>/dev/null || true)
stamped=$(cat "$stamp" 2>/dev/null || true)
if [[ -n $head && $head != "$stamped" ]]; then
  mkdir -p "$data_dir"
  echo "$head" > "$stamp"
  date +%s > "$notice"
  notify "Sorakey updating" "Applying update — the bar restarts for a moment."
  restart_shell
  [[ -n $result ]] && printf 'OK: updating\n' >> "$result"
elif [[ $rc -eq 0 ]]; then
  msg="${last//$id/Sorakey}"
  [[ -n $msg ]] || msg="You're up to date."
  notify "Sorakey" "$msg"
  [[ -n $result ]] && printf 'OK: up-to-date\n' >> "$result"
else
  msg="${last//$id/Sorakey}"
  [[ -n $msg ]] || msg="Update failed — try again."
  notify "Sorakey update failed" "$msg"
  [[ -n $result ]] && printf 'ERROR: %s\n' "$msg" >> "$result"
fi

exit $rc
