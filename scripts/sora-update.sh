#!/bin/bash
# sora-update.sh — Sorakey self-update (panel Update button).
# Must run double-forked via sorakey-detached: a direct child of the panel's
# Process would be SIGTERMed by the reload its own merge triggers.
# $1 = optional result file (sorakey-detached contract: append one line).
#
# Flow: git update -> "applying" notification -> pending-update marker
# (SoraService turns it into the post-restart confirmation) -> detached
# `omarchy restart shell`. The running shell's hot reload never replaces the
# live bar-widget instance, so the restart IS the apply step. The post-merge
# HEAD also goes into the watchdog stamp so the reload-recreated service
# doesn't double-fire.
set -uo pipefail
result="${1:-}"
id="io.github.sandeshrai00.sorakey"
plugin_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data_dir="$HOME/.local/share/sorakey"
stamp="$data_dir/session-head"
notice="$data_dir/pending-update"

notify() { notify-send -a Sorakey "$1" "$2" 2>/dev/null || true; }

rc=0
out=$(omarchy plugin update "$id" --yes 2>&1) || rc=$?
last=$(printf '%s\n' "$out" | tail -n 1)
[[ -n $out ]] && printf '%s\n' "${out//$id/Sorakey}"

if [[ $rc -eq 0 && $out == *"Updated "* ]]; then
  mkdir -p "$data_dir"
  head=$(git -C "$plugin_dir" rev-parse HEAD 2>/dev/null || true)
  [[ -n $head ]] && echo "$head" > "$stamp"
  date +%s > "$notice"
  notify "Sorakey updating" "Applying update — the bar restarts for a moment."
  setsid omarchy restart shell >/dev/null 2>&1 &
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
