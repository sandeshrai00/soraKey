//! Orphan watchdog — `omarchy plugin remove` deletes the plugin checkout with
//! no hook, so nothing stops the user daemon. The unit carries
//! `SORA_PLUGIN_HOME` (written by sora-install); we watch its parent dir with
//! inotify, so the kernel reports the instant our entry is deleted (`rm -rf`,
//! symlink unlink) or moved away (`mv` to backup). The daemon then
//! self-cleans: disable the unit, delete unit + binary, wipe data, exit.
//! The udev rule + live ACL are kept by design (same as the panel's
//! Uninstall button): reinstall or reboot re-asks nothing.
//!
//! A second, independent guard covers the menu-DISABLE case: disabling is a
//! shell.json-only change (the checkout stays), and the shell's teardown of
//! our QML service can be skipped (plugin reload in flight), leaving the
//! daemon running with no panel. The daemon watches ~/.config/omarchy/ with
//! inotify (the shell persists shell.json via atomic rename, so the dir
//! reports it as IN_MOVED_TO — watching the dir, not the file, keeps the
//! watch alive across renames). On a shell.json change: one 3s confirmation
//! read, and if our id is still absent, the daemon disables its own unit and
//! exits (no data wipe — disable is reversible, unlike remove). A poll()
//! timeout on the same fd is the safety net for missed events; if inotify is
//! unavailable at all, a plain 10s poll with ~30s grace applies.
//!
//! Cost: zero idle CPU on both fast paths (the threads sleep in the kernel),
//! one small shell.json parse per config change, no extra resident memory.
//! If inotify is unavailable, the remove path falls back to a 5s stat()
//! poll with ~30s grace.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const TICK: Duration = Duration::from_secs(5);
// Consecutive "gone" poll misses before acting (fallback path only):
// 6 x 5s ~= 30s of grace, so a transient hiccup can never kill a live daemon.
const MISS_LIMIT: u32 = 6;
const UNIT: &str = "sorakey";
// Our plugin id, as it appears in shell.json (the shell's canonicalWidgetId
// is the identity, so a plain string match is exact).
const PLUGIN_ID: &str = "io.github.sandeshrai00.sorakey";
// Disabled-plugin timing: on a shell.json change, one 1s confirmation read
// (a disable->re-enable tap-through rewrites the file, so the second read
// sees the final state; atomic rename means torn reads are impossible).
// SAFETY_TICK is the poll() timeout on the same fd — a missed inotify event
// is still caught: 2 consecutive quiet checks ~= 10s.
const CONFIRM: Duration = Duration::from_secs(1);
const SAFETY_TICK: Duration = Duration::from_secs(5);
const SAFETY_MISS_LIMIT: u32 = 2;
// Inotify unavailable: plain poll with ~30s grace (3 x 10s).
const ENABLE_TICK: Duration = Duration::from_secs(10);
const ENABLE_MISS_LIMIT: u32 = 3;

// linux/inotify.h — entry deletion/move off the parent, plus watch lifecycle.
const IN_DELETE: u32 = 0x200;
const IN_MOVED_FROM: u32 = 0x40;
const IN_IGNORED: u32 = 0x8000;
const IN_Q_OVERFLOW: u32 = 0x4000;
// shell.json writes: atomic rename (IN_MOVED_TO), plain create, in-place write.
const IN_CLOSE_WRITE: u32 = 0x8;
const IN_MOVED_TO: u32 = 0x80;
const IN_CREATE: u32 = 0x100;
const CONFIG_EVENTS: u32 = IN_CLOSE_WRITE | IN_MOVED_TO | IN_CREATE;

fn plugin_home() -> Option<PathBuf> {
    std::env::var_os("SORA_PLUGIN_HOME").map(PathBuf::from)
}

fn gone(home: &Path) -> bool {
    std::fs::metadata(home).is_err()
}

// ---------------------------------------------------------------- disabled

/// The id a shell.json entry carries: layout entries are bare strings or
/// `{ id: … }` objects (the shell accepts both).
fn entry_id(e: &serde_json::Value) -> Option<&str> {
    match e {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Object(_) => e.get("id").and_then(|i| i.as_str()),
        _ => None,
    }
}

/// Mirrors the shell's findEntryLocation: enabled = our id is referenced in
/// `plugins[]` or in any `bar.layout` section.
fn entry_present(v: &serde_json::Value) -> bool {
    if let Some(arr) = v.get("plugins").and_then(|p| p.as_array()) {
        if arr.iter().any(|e| entry_id(e) == Some(PLUGIN_ID)) {
            return true;
        }
    }
    if let Some(layout) = v.get("bar").and_then(|b| b.get("layout")) {
        for section in ["left", "center", "right"] {
            if let Some(arr) = layout.get(section).and_then(|s| s.as_array()) {
                if arr.iter().any(|e| entry_id(e) == Some(PLUGIN_ID)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Pure: None = unparseable (caller fails open), Some(false) = not referenced.
fn enabled_in(text: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    Some(entry_present(&v))
}

fn omarchy_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("omarchy")
}

/// Fail open: no HOME, no file, or a mangled file is "unknown", not
/// "disabled" — a read hiccup must never kill a live daemon.
fn plugin_enabled() -> bool {
    let text = match std::fs::read_to_string(omarchy_dir().join("shell.json")) {
        Ok(t) => t,
        Err(_) => return true,
    };
    enabled_in(&text).unwrap_or(true)
}

/// Disable the unit (a leftover enable symlink would resurrect us at the
/// next user-manager restart) and exit(0) — clean exit, so
/// Restart=on-failure must not revive us. No data wipe: unlike remove, a
/// disable is expected to be reversible.
fn self_disable() {
    crate::always_print!("sorakey: plugin disabled — stopping the daemon");
    let _ = Command::new("systemctl")
        .args(["--user", "disable", UNIT])
        .status();
    std::process::exit(0);
}

/// Watch the shell config dir for shell.json writes. Returns the inotify fd.
/// Dir (not file) so the shell's atomic rename can't orphan the watch.
fn watch_config_dir(dir: &Path) -> Option<RawFd> {
    let cdir = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let fd = unsafe { libc::inotify_init1(libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let wd = unsafe { libc::inotify_add_watch(fd, cdir.as_ptr(), CONFIG_EVENTS) };
    if wd < 0 {
        unsafe { libc::close(fd) };
        return None;
    }
    Some(fd)
}

/// Block until a shell.json event, the SAFETY_TICK timeout, or watch death.
fn wait_config_change(fd: RawFd) -> ConfigChange {
    let mut buf = [0u8; 4096];
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pfd, 1, SAFETY_TICK.as_millis() as libc::c_int) };
        if rc < 0 {
            return ConfigChange::WatchLost;
        }
        if rc == 0 {
            return ConfigChange::Timeout;
        }
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            return ConfigChange::WatchLost;
        }
        match scan_buf(&buf[..n as usize], b"shell.json", CONFIG_EVENTS) {
            Scan::Removed => return ConfigChange::Changed,
            Scan::WatchLost => return ConfigChange::WatchLost,
            Scan::Nothing => continue,
        }
    }
}

/// The shell only tears our QML service down when it thinks of it (a reload
/// in flight suppresses that), so the daemon carries its own exit condition:
/// a shell.json change that leaves us unreferenced = plugin off = stop in
/// ~3s. poll() timeout covers missed events; no inotify at all = plain poll.
pub fn spawn_disabled_watchdog() {
    std::thread::spawn(|| {
        let dir = omarchy_dir();
        if !dir.is_dir() {
            return poll_disabled_fallback();
        }
        let mut safety_misses: u32 = 0;
        loop {
            let fd = match watch_config_dir(&dir) {
                Some(fd) => fd,
                None => return poll_disabled_fallback(),
            };
            let outcome = wait_config_change(fd);
            unsafe { libc::close(fd) };
            match outcome {
                ConfigChange::Changed => {
                    safety_misses = 0;
                    std::thread::sleep(CONFIRM);
                    if !plugin_enabled() {
                        self_disable();
                    }
                }
                ConfigChange::Timeout => {
                    if plugin_enabled() {
                        safety_misses = 0;
                    } else {
                        safety_misses = safety_misses.saturating_add(1);
                        if safety_misses >= SAFETY_MISS_LIMIT {
                            self_disable();
                        }
                    }
                }
                ConfigChange::WatchLost => {
                    // small delay: a pathologically failing watch must not
                    // hot-spin re-arm loops
                    std::thread::sleep(TICK);
                }
            }
        }
    });
}

/// Inotify unavailable: plain poll with ~30s grace.
fn poll_disabled_fallback() {
    let mut misses: u32 = 0;
    loop {
        std::thread::sleep(ENABLE_TICK);
        if plugin_enabled() {
            misses = 0;
        } else {
            misses = misses.saturating_add(1);
            if misses >= ENABLE_MISS_LIMIT {
                self_disable();
            }
        }
    }
}

/// Permission is kept by design (same as the panel's Uninstall button): the
/// udev rule in /etc plus the live ACL survive removal, so a reinstall or
/// reboot re-asks nothing. Revoking needs root with a real terminal prompt,
/// which a detached daemon cannot reliably produce — point at the manual
/// revoke tool instead. Only call when a rule file actually exists.
fn notify_permission_kept() {
    let _ = Command::new("/usr/bin/omarchy-notification-send")
        .args([
            "--app-name",
            "Sorakey",
            "-u",
            "normal",
            "Sorakey removed — keyboard permission kept",
            "Revoke anytime in a terminal with: sudo ~/.local/lib/sorakey/sora-keyboard-revoke.sh",
        ])
        .status();
}

/// Stop + disable our own unit, wipe data, delete unit + binary, then exit(0).
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
        // full data wipe (packs, settings, caches, runtime files) — no .bak,
        // same as the panel's Uninstall button. The staged revoke tool in
        // .local/lib/sorakey is deliberately kept (manual permission revoke).
        for rel in [".local/share/sorakey", ".cache/sorakey"] {
            let p = home.join(rel);
            match std::fs::remove_dir_all(&p) {
                Ok(()) => crate::always_print!("sorakey: removed {}", p.display()),
                Err(e) => {
                    crate::always_eprint!("sorakey: could not remove {}: {e}", p.display())
                }
            }
        }
        let run = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                // SAFETY: getuid has no preconditions.
                let uid = unsafe { libc::getuid() };
                PathBuf::from(format!("/run/user/{uid}"))
            });
        for name in ["sorakey.sock", "sorakey.lock"] {
            let _ = std::fs::remove_file(run.join(name));
            let _ = std::fs::remove_file(home.join(format!(".{name}")));
        }
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        if Path::new("/etc/udev/rules.d/70-sora-keyboard.rules").is_file()
            || Path::new("/etc/udev/rules.d/70-sorakey-keyboard.rules").is_file()
        {
            notify_permission_kept();
        }
    }
    crate::always_print!("sorakey: stopped. Keyboard permission kept by design.");
    std::process::exit(0);
}

/// Call once at startup, before capture starts: plugin already gone or
/// already disabled means a race at boot (unit enabled from an older state,
/// removal during shutdown). One 5s recheck absorbs a half-finished
/// install or a stale shell.json write, then exit so no sound ever plays.
pub fn check_at_startup() {
    if !plugin_enabled() {
        std::thread::sleep(TICK);
        if !plugin_enabled() {
            self_disable();
        }
    }
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

/// Watch the plugin home's parent for deletion/move of our entry.
/// Returns (fd, basename). None = inotify unavailable → caller polls.
/// Parent (not the dir itself) so `rm -rf`, `mv`-to-backup and symlink
/// unlink all report uniformly via the entry name.
fn watch_parent(home: &Path) -> Option<(RawFd, Vec<u8>)> {
    let parent = home.parent()?;
    let base = home.file_name()?.as_bytes().to_vec();
    let cparent = CString::new(parent.as_os_str().as_bytes()).ok()?;
    let fd = unsafe { libc::inotify_init1(libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let wd = unsafe { libc::inotify_add_watch(fd, cparent.as_ptr(), IN_DELETE | IN_MOVED_FROM) };
    if wd < 0 {
        unsafe { libc::close(fd) };
        return None;
    }
    Some((fd, base))
}

#[derive(Debug, PartialEq)]
enum Scan {
    Removed,
    WatchLost,
    Nothing,
}

/// Outcomes of one blocked wait on the shell config watch.
#[derive(Debug, PartialEq)]
enum ConfigChange {
    /// A shell.json write was seen (or the safety timeout fired with the
    /// plugin still referenced — callers re-read to decide).
    Changed,
    /// SAFETY_TICK elapsed with no event: check the config anyway.
    Timeout,
    /// Watch died (overflow, read error): re-arm.
    WatchLost,
}

/// Scan one read() worth of inotify events for `base` under `mask`.
/// Pure: unit-tested with synthetic bytes, no syscalls.
fn scan_buf(buf: &[u8], base: &[u8], mask: u32) -> Scan {
    let mut off = 0;
    while off + 16 <= buf.len() {
        let event_mask =
            u32::from_ne_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
        let len = u32::from_ne_bytes([buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15]])
            as usize;
        if off + 16 + len > buf.len() {
            break; // truncated trailing event: wait for the next read
        }
        let name = &buf[off + 16..off + 16 + len];
        let stem = name.split(|b| *b == 0).next().unwrap_or(&[]);
        if !stem.is_empty() && stem == base && event_mask & mask != 0 {
            return Scan::Removed;
        }
        if event_mask & (IN_IGNORED | IN_Q_OVERFLOW) != 0 {
            return Scan::WatchLost;
        }
        off += 16 + len;
    }
    Scan::Nothing
}

/// Block in the kernel until our entry is deleted/moved or the watch dies.
#[allow(dead_code)]
fn wait_event(fd: RawFd, base: &[u8]) -> Scan {
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            return Scan::WatchLost;
        }
        match scan_buf(&buf[..n as usize], base, IN_DELETE | IN_MOVED_FROM) {
            Scan::Nothing => continue,
            other => return other,
        }
    }
}

/// Same as `wait_event` but with a poll timeout — a missed `IN_DELETE` still
/// triggers a periodic `gone()` re-check so the orphan is not lost forever.
fn wait_event_with_timeout(fd: RawFd, base: &[u8], timeout: Duration) -> Option<Scan> {
    let mut buf = [0u8; 4096];
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as libc::c_int) };
        if rc < 0 {
            return Some(Scan::WatchLost);
        }
        if rc == 0 {
            return None;
        }
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            return Some(Scan::WatchLost);
        }
        match scan_buf(&buf[..n as usize], base, IN_DELETE | IN_MOVED_FROM) {
            Scan::Nothing => continue,
            other => return Some(other),
        }
    }
}

fn poll_fallback(home: &Path) {
    let mut misses: u32 = 0;
    loop {
        std::thread::sleep(TICK);
        if gone(home) {
            misses = misses.saturating_add(1);
            if misses >= MISS_LIMIT {
                self_clean();
            }
        } else {
            misses = 0;
        }
    }
}

/// Spawn the runtime watchdog thread. Returns immediately.
pub fn spawn_watchdog() {
    let home = match plugin_home() {
        Some(h) => h,
        None => return,
    };
    std::thread::spawn(move || {
        // Fast path: the kernel reports the deletion the instant it happens.
        // Re-arm after a false alarm; poll only if inotify is unavailable.
        // ponytail: parent-dir deletion (whole plugins tree wiped) delivers
        // no event — accepted; that never happens via omarchy.
        loop {
            let (fd, base) = match watch_parent(&home) {
                Some(w) => w,
                None => return poll_fallback(&home),
            };
            let outcome = wait_event_with_timeout(fd, &base, Duration::from_secs(30));
            unsafe { libc::close(fd) };
            match outcome {
                None => {
                    if gone(&home) {
                        self_clean();
                    }
                }
                Some(Scan::Removed) | Some(Scan::WatchLost) => {
                    // One re-check: a delete+restore within the same instant
                    // (or a lost watch) must not kill a live daemon.
                    if gone(&home) {
                        self_clean();
                    }
                    // restored, or watch died while home lives: re-arm
                }
                Some(Scan::Nothing) => {} // unreachable
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_EVENTS, IN_CLOSE_WRITE, IN_CREATE, IN_DELETE, IN_MOVED_FROM, IN_MOVED_TO,
        IN_Q_OVERFLOW, MISS_LIMIT, Scan, enabled_in, entry_present, scan_buf, watch_parent,
    };

    const ON: &str = r#"{"bar":{"layout":{"left":[],"center":[],"right":[{"id":"io.github.sandeshrai00.sorakey"}]}},"plugins":[]}"#;

    #[test]
    fn waits_for_sustained_absence() {
        let is_clean = |m: u32| m >= MISS_LIMIT;
        assert!(!is_clean(0));
        assert!(!is_clean(5));
        assert!(is_clean(6));
        assert!(is_clean(100));
    }

    #[test]
    fn enabled_state_from_shell_json() {
        // bar layout object entry (the normal case)
        assert_eq!(enabled_in(ON), Some(true));
        // bar layout bare-string entry
        assert_eq!(
            enabled_in(r#"{"bar":{"layout":{"right":["io.github.sandeshrai00.sorakey"]}}}"#),
            Some(true)
        );
        // plugins[] entry (service-kind placement)
        assert_eq!(
            enabled_in(r#"{"plugins":[{"id":"io.github.sandeshrai00.sorakey"}]}"#),
            Some(true)
        );
        // referenced nowhere = disabled
        assert_eq!(
            enabled_in(
                r#"{"bar":{"layout":{"left":[],"center":[],"right":[{"id":"omarchy.tray"}]}},"plugins":[]}"#
            ),
            Some(false)
        );
        // empty / minimal config
        assert_eq!(enabled_in("{}"), Some(false));
        assert_eq!(enabled_in(r#"{"bar":{"layout":{"left":[]}}}"#), Some(false));
        // unparseable = unknown, not disabled (caller fails open)
        assert_eq!(enabled_in(""), None);
        assert_eq!(enabled_in("not json"), None);
        assert_eq!(enabled_in(r#"{"plugins":}"#), None);
    }

    #[test]
    fn entry_present_needs_the_plugin_id() {
        let v: serde_json::Value = serde_json::from_str(ON).unwrap();
        assert!(entry_present(&v));
        // same shape, different id
        let v: serde_json::Value =
            serde_json::from_str(r#"{"bar":{"layout":{"right":[{"id":"other.plugin"}]}}}"#)
                .unwrap();
        assert!(!entry_present(&v));
    }

    // struct inotify_event { wd:i32, mask:u32, cookie:u32, len:u32, name[] }
    fn ev(mask: u32, name: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&1i32.to_ne_bytes());
        v.extend_from_slice(&mask.to_ne_bytes());
        v.extend_from_slice(&0u32.to_ne_bytes());
        v.extend_from_slice(&(name.len() as u32).to_ne_bytes());
        v.extend_from_slice(name);
        v
    }

    #[test]
    fn parser_spots_delete_or_move_of_our_entry_only() {
        let base = b"io.github.sandeshrai00.sorakey";
        let mut buf = ev(0x200, b"quickshell.spotify\0");
        buf.extend_from_slice(&ev(0x200, b"io.github.sandeshrai00.sorakey\0"));
        assert_eq!(
            scan_buf(&buf, base, IN_DELETE | IN_MOVED_FROM),
            Scan::Removed
        );

        assert_eq!(
            scan_buf(
                &ev(0x200, b"quickshell.spotify\0"),
                base,
                IN_DELETE | IN_MOVED_FROM
            ),
            Scan::Nothing
        );
        assert_eq!(
            scan_buf(
                &ev(0x40, b"io.github.sandeshrai00.sorakey\0"),
                base,
                IN_DELETE | IN_MOVED_FROM
            ),
            Scan::Removed
        );
        assert_eq!(
            scan_buf(&ev(0x8000, b"\0"), base, IN_DELETE | IN_MOVED_FROM),
            Scan::WatchLost
        );
        assert_eq!(
            scan_buf(&[], base, IN_DELETE | IN_MOVED_FROM),
            Scan::Nothing
        );
    }

    #[test]
    fn parser_spots_shell_json_writes_only() {
        let base = b"shell.json";
        // atomic rename (the shell's write path)
        assert_eq!(
            scan_buf(&ev(IN_MOVED_TO, base), base, CONFIG_EVENTS),
            Scan::Removed
        );
        assert_eq!(
            scan_buf(&ev(IN_CREATE, base), base, CONFIG_EVENTS),
            Scan::Removed
        );
        assert_eq!(
            scan_buf(&ev(IN_CLOSE_WRITE, base), base, CONFIG_EVENTS),
            Scan::Removed
        );
        // other entries in the dir / other masks: ignored
        assert_eq!(
            scan_buf(&ev(IN_MOVED_TO, b"plugins\0"), base, CONFIG_EVENTS),
            Scan::Nothing
        );
        assert_eq!(
            // IN_MODIFY (0x2): a write we deliberately do NOT watch
            scan_buf(&ev(0x2, base), base, CONFIG_EVENTS),
            Scan::Nothing
        );
        assert_eq!(
            scan_buf(&ev(IN_Q_OVERFLOW, b"\0"), base, CONFIG_EVENTS),
            Scan::WatchLost
        );
    }

    #[test]
    fn kernel_reports_directory_deletion() {
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("sorakey-orphan-{}", std::process::id()));
        let child = dir.join("child");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&child).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        // detached on purpose: a missing event must fail via the timeouts
        // below, never hang the suite on a blocked join
        let watched = child.clone();
        std::thread::spawn(move || {
            if let Some((fd, base)) = watch_parent(&watched) {
                let _ = ready_tx.send(());
                let out = super::wait_event(fd, &base);
                unsafe { libc::close(fd) };
                let _ = tx.send(out);
            }
        });
        // the watch must exist before the deletion, or the event is lost
        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("watch established within 5s");
        std::fs::remove_dir(&child).unwrap();
        let out = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("inotify DELETE within 5s");
        assert_eq!(out, Scan::Removed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
