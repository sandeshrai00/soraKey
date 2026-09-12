#!/bin/bash
# sora-terminal-grant.sh — runs the keyboard-enable script in sudo mode and
# reports back to the panel, which launched the terminal fire-and-forget and
# otherwise cannot tell approval apart from a closed window.
#
# Files in ${XDG_RUNTIME_DIR:-/tmp}:
#   sorakey-terminal-grant        exit code of the enable script (the result)
#   sorakey-terminal-grant.err    its stderr tail (for the panel's error toast)
#   sorakey-terminal-grant.alive  touched every second while the run is live
# Killed mid-run (window closed, Ctrl-C at the sudo prompt): the trap stops
# the heartbeat and no result file is written — stale/missing .alive with no
# result means the user cancelled.
#
# Usage: sora-terminal-grant.sh /path/to/sora-keyboard-access.sh
# Ends with "Press any key…" so the outcome stays visible before closing.
# set -e is deliberately OFF: a nonzero script exit is a result to report,
# not a reason to die before writing it.
set -u
SCRIPT="${1:?usage: sora-terminal-grant.sh /path/to/sora-keyboard-access.sh}"
RUNDIR="${XDG_RUNTIME_DIR:-/tmp}"
RESULT="$RUNDIR/sorakey-terminal-grant"
ERR="$RESULT.err"
ALIVE="$RESULT.alive"
rm -f "$RESULT" "$ERR" "$ALIVE"
HEART=""
( while true; do touch "$ALIVE" 2>/dev/null; sleep 1; done ) &
HEART=$!
trap 'if [[ -n "$HEART" ]]; then kill "$HEART" 2>/dev/null; fi' EXIT
"$SCRIPT" --use-sudo 2>"$ERR"
code=$?
kill "$HEART" 2>/dev/null
HEART=""
rm -f "$ALIVE"
printf '%s' "$code" > "$RESULT"
echo
read -n1 -rp 'Press any key to close…'
