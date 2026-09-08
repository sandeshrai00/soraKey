use std::process::Command;

/// Absolute binary paths: these run at every daemon start, and a bare
/// `Command::new("systemctl")` would execute whatever `PATH` resolves to.
/// Missing binaries (non-systemd boxes) skip gracefully with None.
const TIMEOUT_BIN: &str = "/usr/bin/timeout";
const SYSTEMCTL_BIN: &str = "/usr/bin/systemctl";

/// Current auto-startup state, or None when it cannot be determined.
///
/// Runs `systemctl --user is-enabled sorakey` under the `timeout` binary so
/// a wedged D-Bus cannot hang daemon startup. When `timeout` is absent we
/// skip gracefully (None) instead of running an unbounded child.
/// Non-systemd boxes / D-Bus errors also yield None so callers keep the
/// user's existing value rather than forcing true -> false.
pub fn get_auto_startup_state() -> Option<bool> {
    // `timeout` present? Skip gracefully when it is not.
    let timeout_ok = Command::new(TIMEOUT_BIN)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !timeout_ok {
        return None;
    }

    let out = Command::new(TIMEOUT_BIN)
        .args(["5", SYSTEMCTL_BIN, "--user", "is-enabled", "sorakey"])
        .output()
        .ok()?;
    // 124 = `timeout` killed the child: state unknown.
    if out.status.code() == Some(124) {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    // (stderr intentionally ignored: any failure output means "unknown")
    if out.status.success() {
        return Some(true);
    }
    match stdout.as_str() {
        "enabled" | "enabled-runtime" => Some(true),
        "disabled" | "linked" | "masked" | "static" | "indirect" | "generated" => Some(false),
        _ => {
            // systemctl failure modes (no systemd, no D-Bus, unit missing
            // with extra error text) and empty/unrecognized output are all
            // detection failures, not "off".
            None
        }
    }
}
