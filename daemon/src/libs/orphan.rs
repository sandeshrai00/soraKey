//! Orphan watchdog — `omarchy plugin remove` deletes the plugin checkout with
//! no hook, so nothing stops the user daemon. The unit carries
//! `SORA_PLUGIN_HOME` (written by sora-install); while that path stays gone
//! the plugin was removed, and the daemon self-cleans: disable the unit,
//! delete unit + binary, exit. Data and the udev rule are left for reinstall.
//!
//! Cost: one stat() per 5s tick plus one idle thread — unmeasurable next to
//! the keyboard rescan the daemon already runs on the same rhythm.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const TICK: Duration = Duration::from_secs(5);
// Consecutive "gone" ticks before acting: 6 x 5s ~= 30s of grace, so a
// transient hiccup can never kill a live daemon. (`plugin update` pulls in
// place, so it never even trips the counter.)
const MISS_LIMIT: u32 = 6;
const UNIT: &str = "sorakey";

fn plugin_home() -> Option<PathBuf> {
    std::env::var_os("SORA_PLUGIN_HOME").map(PathBuf::from)
}

fn gone(home: &Path) -> bool {
    std::fs::metadata(home).is_err()
}

/// Pure decision: act only after sustained absence.
fn should_self_clean(misses: u32) -> bool {
    misses >= MISS_LIMIT
}

/// Stop + disable our own unit, delete unit + binary, then exit(0).
/// exit(0) is load-bearing: Restart=on-failure must NOT revive us.
fn self_clean() {
    crate::always_print!("sorakey: plugin checkout gone — removing orphaned daemon");
    let _ = Command::new("systemctl")
        .args(["--user", "disable", UNIT])
        .status();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for rel in [".config/systemd/user/sorakey.service", ".local/bin/sorakey"] {
            let p = home.join(rel);
            match std::fs::remove_file(&p) {
                Ok(()) => crate::always_print!("sorakey: removed {}", p.display()),
                Err(e) => {
                    crate::always_eprint!("sorakey: could not remove {}: {e}", p.display())
                }
            }
        }
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }
    crate::always_print!(
        "sorakey: stopped. Soundpacks kept at ~/.local/share/sorakey; keyboard rule (if any) can be revoked with: pkexec rm /etc/udev/rules.d/70-sora-keyboard.rules"
    );
    std::process::exit(0);
}

/// Call once at startup, before capture starts: plugin already gone means a
/// remove raced us — one 5s recheck (absorbs a half-finished install), then
/// clean out immediately so no sound ever plays.
pub fn check_at_startup() {
    let home = match plugin_home() {
        Some(h) => h,
        None => return, // old install without the marker: leave it alone
    };
    if !gone(&home) {
        return;
    }
    std::thread::sleep(TICK);
    if gone(&home) {
        self_clean();
    }
}

/// Spawn the runtime watchdog thread. Returns immediately.
pub fn spawn_watchdog() {
    let home = match plugin_home() {
        Some(h) => h,
        None => return,
    };
    std::thread::spawn(move || {
        let mut misses: u32 = 0;
        loop {
            std::thread::sleep(TICK);
            if gone(&home) {
                misses = misses.saturating_add(1);
                if should_self_clean(misses) {
                    self_clean();
                }
            } else {
                misses = 0;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::should_self_clean;

    #[test]
    fn waits_for_sustained_absence() {
        assert!(!should_self_clean(0));
        assert!(!should_self_clean(5));
        assert!(should_self_clean(6));
        assert!(should_self_clean(100));
    }
}
