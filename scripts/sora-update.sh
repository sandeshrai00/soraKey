#!/bin/bash
# sora-update.sh — plugin update + guaranteed UI refresh.
#
# `omarchy plugin update` fetches and fast-forwards the installed clone, but
# the running shell's hot reload keeps the old compiled bar-widget instance
# (its in-memory QML component cache is not invalidated). A detached
# `omarchy restart shell` after a real update is the only reliable way to
# render the new QML; the prebuilt daemon, if any, is re-synced by the
# service freshness check right after the restart.
#
# Last stdout line contract for the panel:
#   "restarting-shell"          — real update done, restart detached
#   "<update output last line>" — already displayable (pluginId → Sorakey)
set -uo pipefail

id="io.github.sandeshrai00.sorakey"
rc=0
out=$(omarchy plugin update "$id" --yes 2>&1) || rc=$?

if [[ -n $out ]]; then
  printf '%s\n' "${out//$id/Sorakey}"
fi

if [[ $rc -eq 0 && $out == *"Updated "* ]]; then
  notify-send -a Sorakey "Sorakey updated" "Restarting shell to apply changes…" || true
  setsid omarchy restart shell >/dev/null 2>&1 &
  echo "restarting-shell"
fi

exit $rc
