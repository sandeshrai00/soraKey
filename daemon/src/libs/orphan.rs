//! Orphan watchdog — `omarchy plugin remove` deletes the plugin checkout with
//! no hook, so nothing stops the user daemon. The unit carries
//! `SORA_PLUGIN_HOME` (written by sora-install); we watch its parent dir with
//! inotify, so the kernel reports the instant our entry is deleted (`rm -rf`,
//! symlink unlink) or moved away (`mv` to backup). The daemon then
//! self-cleans: disable the unit, delete unit + binary, exit. Data and the
//! udev rule are left for reinstall.
//!
//! Cost: zero idle CPU (the thread sleeps in a kernel read) + one thread.
//! If inotify is unavailable, falls back to a 5s stat() poll with ~30s grace.

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

// linux/inotify.h — entry deletion/move off the parent, plus watch lifecycle.
const IN_DELETE: u32 = 0x200;
const IN_MOVED_FROM: u32 = 0x40;
const IN_IGNORED: u32 = 0x8000;
const IN_Q_OVERFLOW: u32 = 0x4000;

fn plugin_home() -> Option<PathBuf> {
    std::env::var_os("SORA_PLUGIN_HOME").map(PathBuf::from)
}

fn gone(home: &Path) -> bool {
    std::fs::metadata(home).is_err()
}

/// Pure decision (fallback path): act only after sustained absence.
fn should_self_clean(misses: u32) -> bool {
    misses >= MISS_LIMIT
}

/// Launch the staged revoke helper fully detached: data wipe + rule/ACL
/// revoke with one approval. Plain spawn is enough — the child is reparented
/// on our exit and the unit is already disabled, so nothing kills it.
/// Absolute binary path: a service context has a minimal PATH.
fn launch_revoke_helper() -> bool {
    let home = match std::env::var_os("HOME").map(PathBuf::from) {
        Some(h) => h,
        None => return false,
    };
    let script = home.join(".local/lib/sorakey/sora-keyboard-revoke.sh");
    if !script.is_file() {
        return false; // old install without the staged helper
    }
    Command::new("/usr/bin/bash")
        .arg(&script)
        .arg("--self-clean")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Surface residue the self-clean could not remove: absolute paths, the
/// panel already documents the same one-liner.
fn notify_residue() {
    let _ = Command::new("/usr/bin/omarchy-notification-send")
        .args([
            "--app-name",
            "Sorakey",
            "-u",
            "normal",
            "Sorakey removed itself",
            "Keyboard permission may remain — revoke with: pkexec rm /etc/udev/rules.d/70-sora-keyboard.rules",
        ])
        .status();
}

/// Stop + disable our own unit, launch the revoke helper, delete unit +
/// binary + consent note, then exit(0).
/// exit(0) is load-bearing: Restart=on-failure must NOT revive us.
fn self_clean() {
    crate::always_print!("sorakey: plugin checkout gone — removing orphaned daemon");
    let _ = Command::new("systemctl")
        .args(["--user", "disable", UNIT])
        .status();
    let helper_launched = launch_revoke_helper();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for rel in [
            ".config/systemd/user/sorakey.service",
            ".local/bin/sorakey",
            // consent note: reinstall re-asks even if the OS grant survived
            ".local/share/sorakey/keyboard-granted",
        ] {
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
    if helper_launched {
        crate::always_print!(
            "sorakey: stopped. Revoke helper launched: full data wipe + keyboard permission revoke with one approval."
        );
    } else {
        notify_residue();
        crate::always_print!(
            "sorakey: stopped. No staged revoke helper — keyboard permission may remain (see notification); rule (if any) can be revoked with: pkexec rm /etc/udev/rules.d/70-sora-keyboard.rules"
        );
    }
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

/// Scan one read() worth of inotify events for our basename.
/// Pure: unit-tested with synthetic bytes, no syscalls.
fn scan_buf(buf: &[u8], base: &[u8]) -> Scan {
    let mut off = 0;
    while off + 16 <= buf.len() {
        let mask = u32::from_ne_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
        let len = u32::from_ne_bytes([buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15]])
            as usize;
        if off + 16 + len > buf.len() {
            break; // truncated trailing event: wait for the next read
        }
        let name = &buf[off + 16..off + 16 + len];
        let stem = name.split(|b| *b == 0).next().unwrap_or(&[]);
        if !stem.is_empty() && stem == base && mask & (IN_DELETE | IN_MOVED_FROM) != 0 {
            return Scan::Removed;
        }
        if mask & (IN_IGNORED | IN_Q_OVERFLOW) != 0 {
            return Scan::WatchLost;
        }
        off += 16 + len;
    }
    Scan::Nothing
}

/// Block in the kernel until our entry is deleted/moved or the watch dies.
fn wait_event(fd: RawFd, base: &[u8]) -> Scan {
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            return Scan::WatchLost;
        }
        match scan_buf(&buf[..n as usize], base) {
            Scan::Nothing => continue,
            other => return other,
        }
    }
}

fn poll_fallback(home: &Path) {
    let mut misses: u32 = 0;
    loop {
        std::thread::sleep(TICK);
        if gone(home) {
            misses = misses.saturating_add(1);
            if should_self_clean(misses) {
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
            let outcome = wait_event(fd, &base);
            unsafe { libc::close(fd) };
            match outcome {
                Scan::Removed | Scan::WatchLost => {
                    // One re-check: a delete+restore within the same instant
                    // (or a lost watch) must not kill a live daemon.
                    if gone(&home) {
                        self_clean();
                    }
                    // restored, or watch died while home lives: re-arm
                }
                Scan::Nothing => {} // unreachable: wait_event only returns terminal scans
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Scan, scan_buf, should_self_clean, watch_parent};

    #[test]
    fn waits_for_sustained_absence() {
        assert!(!should_self_clean(0));
        assert!(!should_self_clean(5));
        assert!(should_self_clean(6));
        assert!(should_self_clean(100));
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
        assert_eq!(scan_buf(&buf, base), Scan::Removed);

        assert_eq!(
            scan_buf(&ev(0x200, b"quickshell.spotify\0"), base),
            Scan::Nothing
        );
        assert_eq!(
            scan_buf(&ev(0x40, b"io.github.sandeshrai00.sorakey\0"), base),
            Scan::Removed
        );
        assert_eq!(scan_buf(&ev(0x8000, b"\0"), base), Scan::WatchLost);
        assert_eq!(scan_buf(&[], base), Scan::Nothing);
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
