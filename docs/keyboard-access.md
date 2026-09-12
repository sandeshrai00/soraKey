# Keyboard access — how Sorakey hears keys

Sorakey plays sounds as you type. To do that, it needs to read keyboard events.

## Why approval is needed

Linux locks keyboard input so only your active login session can read `/dev/input/event*`. Sorakey runs as a user service in the background, so the system asks you — once — to grant it access. This is the same kind of one-time approval every app install asks for, and it works with password or fingerprint. The approval is handled by your system’s polkit agent (GUI dialog) or `sudo` (terminal) — Sorakey never sees what you type.

## What tapping “Enable keyboard sounds” does

1. Your system’s approval dialog appears (not ours).
2. On approval, one scoped udev rule is installed **for keyboards only**:
   `/etc/udev/rules.d/70-sora-keyboard.rules`
   ```ini
   ACTION=="add|change", SUBSYSTEM=="input", ENV{ID_INPUT_KEYBOARD}=="1", TAG+="uaccess"
   ```
   - `TAG+="uaccess"` tells `systemd-logind` to add a per-user ACL on the matching keyboard event nodes (e.g. `/dev/input/event4`). Only the active session user gets `rw`, not everyone.
   - Mice, touchpads, joysticks are untouched (`ID_INPUT_KEYBOARD==1` only).
   - Must sort **before** `73-seat-late.rules` so the `uaccess` builtin actually writes the ACL at event time.
3. `udevadm control --reload-rules && udevadm trigger --subsystem-match=input --action=change` applies it instantly — no logout, no reboot. Sounds start within seconds. Panel shows `Checking status…` → `Enabling… / Check your terminal…` → `Finishing up…` → `Playing`.

Source: `udev/70-sora-keyboard.rules` and `scripts/sora-keyboard-access.sh` in the plugin directory.

## What “Enable keyboard permission with terminal” does

Same rule, same `install + reload + trigger`, but via `sudo` in your own terminal instead of `pkexec` GUI dialog. Use it when:

- Your system has no polkit agent (no GUI approval dialog), or
- You cancelled the GUI dialog and prefer the terminal.

The panel opens your `$TERMINAL` (or `xdg-terminal-exec`) and runs:
```bash
/path/to/plugin/scripts/sora-keyboard-access.sh --use-sudo
# you type your password in your own terminal, nothing is logged
```
Both buttons share the same unified `Enabling…` UI — both buttons hide, one centered spinner + `Check your terminal… / Waiting for approval…` → `Verifying…` → `Finishing up…`.

Manual equivalent (run from plugin directory):
```bash
sudo install -m 644 udev/70-sora-keyboard.rules /etc/udev/rules.d/70-sora-keyboard.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input --action=change
# verify: should show user:<you>:rw- on a keyboard node, and daemon status ok
getfacl /dev/input/event4 2>/dev/null | grep user:
~/.local/bin/sorakey ctl '{"cmd":"status"}' | grep input_error
# expected: "input_error":null
```

## How to verify permission

```bash
# 1. Rule installed?
ls -l /etc/udev/rules.d/70-sora-keyboard.rules

# 2. ACL granted to you? (should show user:<you>:rw- on a keyboard node)
for d in /dev/input/event*; do
  udevadm info --query=property --name="$d" 2>/dev/null | grep -q "ID_INPUT_KEYBOARD=1" && getfacl "$d" | grep -E "^user:"
done

# 3. Daemon sees keyboards?
~/.local/bin/sorakey ctl '{"cmd":"status"}' | python3 -m json.tool | grep -A1 input_error
# null = OK, string = blocked: "no_input_devices: cannot open /dev/input/event*"

# Panel should show "Playing" / "Muted", not "No keyboard access"
```

## How to remove keyboard permission (for testing or privacy)

Removing the rule revokes future access; revoking the ACL on live nodes stops the current session too. The daemon must be restarted to drop its open file descriptors.

### Option A — via GUI (recommended for users)

Uninstalling the plugin removes the rule **and revokes the live ACL** with the same one-time approval (so a same-session reinstall asks for keyboard permission again):

```bash
# from the panel: Settings → Uninstall Sorakey, or:
~/.config/omarchy/plugins/io.github.sandeshrai00.sorakey/scripts/sora-uninstall.sh --purge
# then: omarchy plugin remove io.github.sandeshrai00.sorakey --yes
```

`sora-uninstall.sh` revokes the full permission in one approved step: it removes both rule files (`70-sora-keyboard.rules` + legacy), reloads and re-triggers udev, and runs `setfacl -b` on the current keyboard nodes only (`ID_INPUT_KEYBOARD==1`; mice/touchpads keep their grants). Removing the rule alone would leave a stale ACL that hides the re-install prompt — the rule file would then stay missing and the keyboard would be dead after the next reboot.

### Option B — manual revoke (for developers / fresh-install testing)

No terminal is available to agents, so the shell uses `pkexec` (GUI prompt). In a terminal you can use `sudo`:

```bash
# 1. Remove rule
pkexec bash -c 'rm -f /etc/udev/rules.d/70-sora-keyboard.rules && udevadm control --reload-rules && udevadm trigger --subsystem-match=input --action=change'
# — or with sudo in a terminal:
# sudo rm /etc/udev/rules.d/70-sora-keyboard.rules
# sudo udevadm control --reload-rules
# sudo udevadm trigger --subsystem-match=input --action=change

# 2. Revoke live ACLs on current keyboard nodes (udev trigger revokes future nodes, setfacl clears current ones)
pkexec bash -c 'setfacl -b /dev/input/event* 2>/dev/null; udevadm trigger /dev/input/event4 2>/dev/null || true'
# — or: sudo setfacl -b /dev/input/event* 2>/dev/null

# 3. Restart daemon so it re-opens devices (now denied) and panel so it re-evaluates
systemctl --user restart sorakey
omarchy restart shell

# 4. Verify revoked
ls /etc/udev/rules.d/70-sora-keyboard.rules 2>&1 | head   # should be "No such file"
getfacl /dev/input/event4 2>/dev/null | grep "^user:"        # should be only user::rw-, no user:<you>:rw-
~/.local/bin/sorakey ctl '{"cmd":"status"}' | grep input_error  # should be "no_input_devices: cannot open /dev/input/event*"
# Panel should show: Checking status… (spinning) → NEEDS ATTENTION / Enable keyboard sounds
```

Re-grant anytime: open the Sorakey panel → **Enable keyboard sounds** (GUI) or **Enable keyboard permission with terminal** (sudo) — one approval restores the rule + ACL instantly.

### Fresh-install test sequence (what we use)

```bash
# revoke (as above)
pkexec bash -c 'rm -f /etc/udev/rules.d/70-sora-keyboard.rules /etc/udev/rules.d/70-sorakey-keyboard.rules && udevadm control --reload-rules && udevadm trigger --subsystem-match=input --action=change; setfacl -b /dev/input/event* 2>/dev/null'
systemctl --user restart sorakey; omarchy restart shell
# open panel → expect: Checking (spinner) → Need Attention, no main flash
# click Enable → expect: both buttons hide → centered "Enabling…" + "Waiting for approval…" / "Check your terminal…" → "Finishing up…" → main controls
```

## Privacy

Keys are heard only to play sounds:

- never recorded,
- never logged (the sound engine is tested to stay silent in code),
- never sent anywhere (everything stays on your machine).

## FAQ

**I tapped Cancel — now what?**
Nothing breaks. The panel keeps showing the honest state. Tap **Enable keyboard sounds** again whenever you’re ready. The terminal button remains as fallback.

**Will I be asked again?**
No. The rule persists across reboots and `omarchy update` until you remove it.

**Does it slow down typing or drain battery?**
No. Key detection is event-driven (zero polling), and each keystroke reuses precomputed audio — no work per key beyond playback.

**I use a different layout / multiple keyboards.**
Access covers all keyboards on the system, and key detection is layout-independent.

**No approval dialog appeared?**
Your system has no polkit agent. Use **Enable keyboard permission with terminal** — it opens your terminal and runs the same step with `sudo`.

## How it works under the hood

```
Panel (QML) ──enableCapture()/fixInTerminal()──▶ sora-keyboard-access.sh
                                                     │ pkexec/sudo
                                                     ▼
                                              /etc/udev/rules.d/70-sora-keyboard.rules (TAG+=uaccess)
                                                     │ udevadm reload + trigger
                                                     ▼
                                              73-seat-late.rules builtin "uaccess" → ACL on /dev/input/event* (user:<you>:rw-)
                                                     │ daemon re-opens on next enumerate (5s) or restart
                                                     ▼
                                              sorakey daemon (evdev) → sound engine → Pulse/PipeWire
```

- Panel state: `statusKnown` gates the UI — `Checking status…` (spinning) until `sorakey ctl status` returns a definitive `input_error` (string = blocked, `null` = clear after 2 consecutive polls to avoid the daemon’s startup lie-window). Then either `Need Attention` or main controls. See `SoraWidget.qml:92-98,213,1304-1345`.
- Daemon health: `daemon/src/state/status.rs` (`input_error` slot) + `daemon/src/libs/keyboard.rs:192-265` (enumerate retry, supervisor).
