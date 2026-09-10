#!/bin/bash
# sora-update.sh — plugin update + guaranteed UI refresh.
# Must run double-forked via sorakey-detached: as a direct child of the
# panel's Process it gets SIGTERMed by the reload its own merge triggers.
# $1 = optional result file (sorakey-detached contract: append one line).
#
# The running shell's hot reload never replaces the live bar-widget
# instance, so a real update ends in a detached `omarchy restart shell`.
# The post-merge HEAD goes into the service watchdog's stamp file so the
# reload-recreated service doesn't double-fire.
set -uo pipefail
result="${1:-}"
id="io.github.sandeshrai00.sorakey"
plugin_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stamp="$HOME/.local/share/sorakey/session-head"

rc=0
out=$(omarchy plugin update "$id" --yes 2>&1) || rc=$?
last=$(printf '%s\n' "$out" | tail -n 1)
[[ -n $out ]] && printf '%s\n' "${out//$id/Sorakey}"

if [[ $rc -eq 0 && $out == *"Updated "* ]]; then
  mkdir -p "$(dirname "$stamp")"
  git -C "$plugin_dir" rev-parse HEAD > "$stamp" 2>/dev/null || true
  notify-send -a Sorakey "Sorakey updated" "Restarting shell to apply changes…" || true
  setsid omarchy restart shell >/dev/null 2>&1 &
  [[ -n $result ]] && printf 'OK: restarting-shell\n' >> "$result"
elif [[ $rc -eq 0 ]]; then
  msg="${last//$id/Sorakey}"
  [[ -n $msg ]] || msg="no update available"
  notify-send -a Sorakey "Sorakey up to date" "$msg" || true
  [[ -n $result ]] && printf 'OK: up-to-date\n' >> "$result"
else
  notify-send -a Sorakey "Sorakey update failed" "${last:-see terminal for details}" || true
  [[ -n $result ]] && printf 'ERROR: %s\n' "${last:-update failed}" >> "$result"
fi

exit $rc
