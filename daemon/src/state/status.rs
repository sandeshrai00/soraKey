//! Runtime health shared between input listener, engine, and control API.
//! Explains "daemon running but silent": input permission, pack load, audio.
//! All atomics/locks — safe to call from any thread, never blocks the hot path.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static KEYBOARDS: AtomicUsize = AtomicUsize::new(0);
static LAST_KEY_UNIX: AtomicU64 = AtomicU64::new(0);
static PACK_STATE: AtomicU8 = AtomicU8::new(0); // 0=unknown, 1=loaded, 2=failed
static AUDIO_OK: AtomicBool = AtomicBool::new(false);

fn input_error_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn pack_error_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn audio_error_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Last seen OS default-sink name (follow-default bookkeeping). `None` also
/// counts: "no default device" is a state worth noticing exactly once.
fn default_sink_slot() -> &'static Mutex<Option<Option<String>>> {
    static SLOT: OnceLock<Mutex<Option<Option<String>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// What actually opened: `None` = system default, `Some(id)` = explicit
/// device. Written by the engine on open/switch; read by `status` so the
/// panel never shows a selection the stream isn't on.
fn opened_device_slot() -> &'static Mutex<Option<Option<String>>> {
    static SLOT: OnceLock<Mutex<Option<Option<String>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Input listener reports how many keyboards it holds.
pub fn set_input_keyboards(n: usize) {
    KEYBOARDS.store(n, Ordering::Relaxed);
}

/// Input listener reports a problem (e.g. no /dev/input access).
/// `None` clears the error.
pub fn set_input_error(err: Option<String>) {
    if let Ok(mut slot) = input_error_slot().lock() {
        *slot = err;
    }
}

/// Record that a key event reached the engine (call on keydown).
pub fn note_key() {
    LAST_KEY_UNIX.store(now_unix(), Ordering::Relaxed);
}

/// Engine reports pack load outcome.
pub fn set_pack_result(loaded: bool, err: Option<String>) {
    PACK_STATE.store(if loaded { 1 } else { 2 }, Ordering::Relaxed);
    if let Ok(mut slot) = pack_error_slot().lock() {
        *slot = err;
    }
}

/// Engine reports audio backend health.
pub fn set_audio_result(ok: bool, err: Option<String>) {
    AUDIO_OK.store(ok, Ordering::Relaxed);
    if let Ok(mut slot) = audio_error_slot().lock() {
        *slot = err;
    }
}

/// Records the OS default-sink name seen by this status poll. Returns true
/// exactly once per change (first sighting does NOT count — the engine seeds
/// this at boot with what it opened, so a fresh daemon doesn't reopen on
/// its first poll). The caller reopens the default stream on true.
pub fn note_default_sink(current: Option<String>) -> bool {
    if let Ok(mut slot) = default_sink_slot().lock() {
        match slot.as_ref() {
            // Unseeded (engine predates this / tests): seed silently.
            None => {
                *slot = Some(current);
                return false;
            }
            Some(prev) if *prev == current => return false,
            _ => {
                *slot = Some(current);
                return true;
            }
        }
    }
    false
}

/// Engine reports which device the live stream is actually on.
pub fn set_opened_device(opened: Option<String>) {
    if let Ok(mut slot) = opened_device_slot().lock() {
        *slot = Some(opened);
    }
}

/// The device the live stream is on (`None` = system default, outer `None` =
/// engine hasn't reported yet).
pub fn opened_device() -> Option<Option<String>> {
    opened_device_slot().lock().ok().and_then(|g| g.clone())
}

fn get_opt(slot: &'static Mutex<Option<String>>) -> Option<String> {
    slot.lock().ok().and_then(|g| g.clone())
}

/// Snapshot merged into `status` and `diag` responses.
pub fn snapshot() -> serde_json::Value {
    let now = now_unix();
    let last = LAST_KEY_UNIX.load(Ordering::Relaxed);
    serde_json::json!({
        "input_keyboards": KEYBOARDS.load(Ordering::Relaxed),
        "input_error": get_opt(input_error_slot()),
        "last_key_age_s": if last == 0 { serde_json::Value::Null } else { serde_json::json!(now.saturating_sub(last)) },
        "pack_loaded": match PACK_STATE.load(Ordering::Relaxed) {
            1 => serde_json::json!(true),
            2 => serde_json::json!(false),
            _ => serde_json::Value::Null,
        },
        "pack_error": get_opt(pack_error_slot()),
        "audio_ok": AUDIO_OK.load(Ordering::Relaxed),
        "audio_error": get_opt(audio_error_slot()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_snapshot_reports_what_was_set() {
        set_input_keyboards(2);
        set_input_error(Some("test error".to_string()));
        set_pack_result(false, Some("pack boom".to_string()));
        set_audio_result(true, None);
        let s = snapshot();
        assert_eq!(s["input_keyboards"], 2);
        assert_eq!(s["input_error"], "test error");
        assert_eq!(s["pack_loaded"], false);
        assert_eq!(s["pack_error"], "pack boom");
        assert_eq!(s["audio_ok"], true);
        // reset for other tests
        set_input_keyboards(0);
        set_input_error(None);
        PACK_STATE.store(0, Ordering::Relaxed);
        set_pack_result(true, None);
    }

    #[test]
    fn default_sink_change_fires_exactly_once() {
        // First sighting seeds silently (fresh daemon must not reopen on
        // its first poll); a change fires once; repeats stay quiet.
        // NOTE: global slot shared with other tests (a commands test drives
        // `status`), so force a known state first instead of assuming fresh.
        // Unique names keep parallel tests from colliding.
        note_default_sink(Some("test-sink-gamma-base".to_string()));
        assert!(!note_default_sink(Some("test-sink-gamma-base".to_string())));
        assert!(note_default_sink(Some("test-sink-gamma-next".to_string())));
        assert!(!note_default_sink(Some("test-sink-gamma-next".to_string())));
        assert!(note_default_sink(None));
        assert!(!note_default_sink(None));
        // leave seeded; engine re-seeds at boot anyway.
    }

    #[test]
    fn opened_device_round_trips() {
        set_opened_device(None);
        assert_eq!(opened_device(), Some(None));
        set_opened_device(Some("test-device".to_string()));
        assert_eq!(opened_device(), Some(Some("test-device".to_string())));
    }
}
