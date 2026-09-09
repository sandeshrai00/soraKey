//! Control API over Unix socket (`$XDG_RUNTIME_DIR/sorakey.sock`).
//! One JSON line in, one out. All writes go through config_writer + engine.

use crate::libs::names::qualify_soundpack_id;
use crate::libs::player::{AudioCommand, AudioEngineHandle};
use crate::state::folders;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub fn socket_path() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) => PathBuf::from(dir).join("sorakey.sock"),
        Err(_) => std::env::temp_dir()
            .join("sorakey.sock")
            .join(".sorakey.sock"),
    }
}

/// Spawn the accept loop. Returns the bound socket path.
pub fn serve(engine: AudioEngineHandle) -> Option<PathBuf> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            crate::always_eprint!("❌ [control] cannot bind {}: {}", path.display(), e);
            return None;
        }
    };
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let eng = engine.clone();
                    let _ = std::thread::Builder::new()
                        .stack_size(64 * 1024)
                        .spawn(move || {
                            let _ = stream
                                .set_read_timeout(Some(std::time::Duration::from_millis(100)));
                            handle_conn(stream, &eng)
                        });
                }
                Err(e) => crate::always_print!("⚠️  [control] accept: {}", e),
            }
        }
    });

    Some(path)
}

fn handle_conn(mut stream: UnixStream, engine: &AudioEngineHandle) {
    const MAX_REQUEST_BYTES: u64 = 64 * 1024;
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 4096];
    let mut reader = BufReader::new(&stream);
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if (buf.len() as u64 + n as u64) > MAX_REQUEST_BYTES {
                    let _ = stream.write_all(b"{\"ok\":false,\"error\":\"request too large\"}\n");
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(_) => {
                // Timeout or disconnect mid-request. A bare connect-close
                // probe (empty buf) stays silent; a partial request gets an
                // error line instead of a silent drop.
                if buf.iter().any(|b| !b.is_ascii_whitespace()) {
                    let _ = stream.write_all(fail("incomplete request").as_bytes());
                    let _ = stream.write_all(b"\n");
                }
                return;
            }
        }
        if buf.contains(&b'\n') {
            break;
        }
    }
    let Some(newline) = buf.iter().position(|b| *b == b'\n') else {
        // EOF/timeout with a partial line: answer instead of dropping silently.
        if buf.iter().any(|b| !b.is_ascii_whitespace()) {
            let _ = stream.write_all(fail("incomplete request").as_bytes());
            let _ = stream.write_all(b"\n");
        }
        return;
    };
    let line = String::from_utf8_lossy(&buf[..newline]).into_owned();
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let response = dispatch(line, engine);
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(b"\n");
}

fn dispatch(request: &str, engine: &AudioEngineHandle) -> String {
    let req: serde_json::Value = match serde_json::from_str(request) {
        Ok(v) => v,
        Err(e) => return fail(&format!("bad json: {e}")),
    };
    let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("");

    match cmd {
        "status" => status(engine),
        "set_bar_section" => set_bar_section(&req),
        "get_bar_section" => get_bar_section(),
        "mute" => {
            let muted = req.get("muted").and_then(|v| v.as_bool()).unwrap_or(true);
            crate::state::settings_saver::apply(|c| c.enable_sound = !muted);
            engine.send(AudioCommand::SetSoundEnabled(!muted));
            ok(serde_json::json!({ "muted": muted }))
        }
        "volume" => {
            let v = match clamp_percent(req.get("value")) {
                Some(v) => v,
                None => return fail("value must be 0-100"),
            };
            let f = v / 100.0;
            // Master slider always moves the global default; per-pack
            // overrides are only touched via the per_pack_volume cmd.
            crate::state::settings_saver::apply(|c| c.volume = f);
            let eff = crate::state::settings_saver::current().effective_volume();
            engine.send(AudioCommand::SetVolume(eff));
            ok(serde_json::json!({ "volume": v }))
        }
        "per_pack_volume" => per_pack_volume(&req, engine),
        "reset_volume" => reset_volume(&req, engine),
        "delete_pack" => delete_pack(&req, engine),
        "keyboard_pack" => load_pack(&req, engine),
        "packs" => packs(),
        "audio_devices" => audio_devices(),
        "select_device" => select_device(&req, engine),
        "diag" => diag(),
        "export_logs" => export_logs(),
        "key" => key_event(&req, engine),
        "toggle_mute" => toggle_mute(engine),
        other => fail(&format!("unknown cmd: {other}")),
    }
}

fn clamp_percent(v: Option<&serde_json::Value>) -> Option<f32> {
    let n = v?.as_f64()?;
    if !(0.0..=100.0).contains(&n) {
        return None;
    }
    Some(n as f32)
}

fn status(engine: &AudioEngineHandle) -> String {
    let c = crate::state::settings_saver::current();
    let eff = c.effective_volume();
    let per = c
        .per_pack_volume
        .get(&c.keyboard_soundpack)
        .copied()
        .unwrap_or(eff);
    // Follow the OS default sink while "System Default" is selected: the
    // engine pins its stream to whatever sink was default at open (daemon
    // start often wins that race against headsets), so a later default-sink
    // change must reopen the stream or sound stays on the old sink until a
    // restart. One cheap default-name query per poll (no enumeration — that
    // would stall keystrokes); fires at most once per change, and a failed
    // reopen keeps the old stream playing (switch_device semantics).
    if c.selected_audio_device.is_none()
        && crate::state::status::note_default_sink(
            crate::libs::speakers::DeviceManager::new().default_output_name(),
        )
        && !engine.send(AudioCommand::SwitchDevice(None))
    {
        crate::always_eprint!("⚠️ [status] default sink changed but engine is unavailable");
    }
    let mut v = serde_json::json!({
        "running": true,
        "muted": !c.enable_sound,
        "volume": (eff * 100.0).round(),
        "per_pack_volume": (per * 100.0).round(),
        "keyboard_pack": c.keyboard_soundpack,
        "audio_device": c.selected_audio_device,
        // What the live stream is actually on: None = system default. The
        // panel must treat a mismatch with `audio_device` as "waiting for
        // device", never as playing on the selection.
        "audio_device_opened": crate::state::status::opened_device(),
    });
    // Health explains "running but silent" (input capture, pack load, audio).
    if let Some(obj) = v.as_object_mut() {
        for (k, val) in crate::state::status::snapshot()
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            obj.insert(k, val);
        }
    }
    ok(v)
}

/// Fast key ingest for notifiers that only hold an engine handle.
fn key_event(req: &serde_json::Value, engine: &AudioEngineHandle) -> String {
    let code = match req.get("code").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return fail("missing code"),
    };
    if code.len() > 32
        || !code.bytes().all(|b| b.is_ascii_alphanumeric())
        || !is_known_key_code(code)
    {
        return fail("unknown code");
    }
    let down = req.get("down").and_then(|v| v.as_bool()).unwrap_or(true);
    engine.send(AudioCommand::Key {
        code: code.to_string(),
        down,
    });
    ok(serde_json::json!({ "code": code, "down": down }))
}

fn is_known_key_code(code: &str) -> bool {
    if crate::utils::keys::KEY_MAP.iter().any(|&(_, n)| n == code) {
        return true;
    }
    matches!(
        code,
        "ControlRight"
            | "AltRight"
            | "MetaLeft"
            | "MetaRight"
            | "ArrowUp"
            | "ArrowDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "Insert"
            | "Delete"
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
            | "PrintScreen"
            | "Pause"
            | "ScrollLock"
            | "NumLock"
            | "CapsLock"
            | "ContextMenu"
            | "Power"
            | "Sleep"
            | "WakeUp"
            | "Fn"
            | "Clear"
            | "Help"
            | "Props"
            | "Front"
            | "Stop"
            | "Again"
            | "Undo"
            | "Cut"
            | "Copy"
            | "Paste"
            | "Find"
            | "F13"
            | "F14"
            | "F15"
            | "F16"
            | "F17"
            | "F18"
            | "F19"
            | "F20"
            | "F21"
            | "F22"
            | "F23"
            | "F24"
            | "NumpadEnter"
            | "NumpadDivide"
            | "NumpadEquals"
            | "NumpadComma"
            | "Convert"
            | "Lang1"
            | "Lang2"
            | "KanaMode"
            | "HiraganaKatakana"
            | "IntlYen"
            | "IntlBackslash"
            | "MediaTrackPrevious"
            | "MediaTrackNext"
            | "MediaPlayPause"
            | "MediaStop"
            | "MediaSelect"
            | "AudioVolumeMute"
            | "AudioVolumeDown"
            | "AudioVolumeUp"
            | "BrowserHome"
            | "BrowserSearch"
            | "BrowserFavorites"
            | "BrowserRefresh"
            | "BrowserStop"
            | "BrowserForward"
            | "BrowserBack"
            | "LaunchApp1"
            | "LaunchApp2"
            | "LaunchApp3"
            | "LaunchMail"
    )
}

fn toggle_mute(engine: &AudioEngineHandle) -> String {
    let mut enabled = false;
    crate::state::settings_saver::apply(|config| {
        config.enable_sound = !config.enable_sound;
        enabled = config.enable_sound;
    });
    engine.send(AudioCommand::SetSoundEnabled(enabled));
    crate::always_print!("🔄 [control] Sound toggled: {}", enabled);
    ok(serde_json::json!({ "muted": !enabled }))
}

fn recommended_volume_for(id: &str) -> Option<f32> {
    let dir = folders::soundpacks::contained_dir(id)?;
    let path = folders::soundpacks::contained_file(&dir, "config.json")?;
    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("options")?
        .get("recommended_volume")?
        .as_f64()
        .map(|n| n as f32)
}

fn load_pack(req: &serde_json::Value, engine: &AudioEngineHandle) -> String {
    let raw = match req.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return fail("missing id"),
    };
    if raw.contains('\0') || raw.contains("..") {
        return fail("invalid id");
    }
    let id = qualify_soundpack_id(&raw, "keyboard/");
    // Verify the pack exists (directory + readable config) before claiming ok.
    // contained_dir fails closed on symlink escapes, like delete_pack.
    {
        let dir = match folders::soundpacks::contained_dir(&id) {
            Some(d) => d,
            None => return fail("invalid path"),
        };
        let cfg = match folders::soundpacks::contained_file(&dir, "config.json") {
            Some(c) => c,
            None => return fail("invalid path"),
        };
        if !dir.is_dir() || std::fs::read_to_string(&cfg).is_err() {
            return fail("pack not found");
        }
    }
    let rec = recommended_volume_for(&id);
    let will_insert = rec.is_some_and(|v| {
        let v = v.clamp(0.1, 1.0);
        (v - 1.0).abs() > 0.001
    }) && !crate::state::settings_saver::current()
        .per_pack_volume
        .contains_key(&id);
    if will_insert && too_many_per_pack_entries(&id) {
        return fail("too many per-pack entries");
    }

    crate::state::settings_saver::apply(|c| {
        c.keyboard_soundpack = id.clone();
        if !c.per_pack_volume.contains_key(&id) {
            if let Some(v) = rec {
                let v = v.clamp(0.1, 1.0);
                if (v - 1.0).abs() > 0.001 {
                    c.per_pack_volume.insert(id.clone(), v);
                }
            }
        }
    });
    let eff = crate::state::settings_saver::current().effective_volume();
    engine.send(AudioCommand::LoadKeyboardPack {
        soundpack_id: id.clone(),
        update_cache_on_error: true,
    });
    engine.send(AudioCommand::SetVolume(eff));
    ok(serde_json::json!({ "id": id }))
}

fn reset_volume(req: &serde_json::Value, engine: &AudioEngineHandle) -> String {
    let raw = match req.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return fail("missing id"),
    };
    if raw.contains('\0') || raw.contains("..") {
        return fail("invalid id");
    }
    let id = qualify_soundpack_id(&raw, "keyboard/");
    crate::state::settings_saver::apply(|c| {
        c.per_pack_volume.remove(&id);
        if let Some(v) = recommended_volume_for(&id) {
            let v = v.clamp(0.1, 1.0);
            if (v - 1.0).abs() > 0.001 {
                c.per_pack_volume.insert(id.clone(), v);
            }
        }
    });
    let cur = crate::state::settings_saver::current();
    if cur.keyboard_soundpack == id {
        engine.send(AudioCommand::SetVolume(cur.effective_volume()));
    }
    let per = cur.per_pack_volume.get(&id).copied().unwrap_or(cur.volume) * 100.0;
    ok(serde_json::json!({ "id": id, "per_pack_volume": per.round() }))
}

fn per_pack_volume(req: &serde_json::Value, engine: &AudioEngineHandle) -> String {
    let raw = match req.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return fail("missing id"),
    };
    if raw.contains('\0') || raw.contains("..") {
        return fail("invalid id");
    }
    let id = qualify_soundpack_id(&raw, "keyboard/");
    if too_many_per_pack_entries(&id) {
        return fail("too many per-pack entries");
    }
    let v = match clamp_percent(req.get("value")) {
        Some(v) => v,
        None => return fail("value must be 0-100"),
    };
    let f = (v / 100.0).clamp(0.0, 1.0);
    crate::state::settings_saver::apply(|c| {
        c.per_pack_volume.insert(id.clone(), f);
    });
    let cur = crate::state::settings_saver::current();
    if cur.keyboard_soundpack == id {
        let eff = cur.effective_volume();
        engine.send(AudioCommand::SetVolume(eff));
    }
    ok(serde_json::json!({ "id": id, "per_pack_volume": v }))
}

fn delete_pack(req: &serde_json::Value, engine: &AudioEngineHandle) -> String {
    let raw = match req.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return fail("missing id"),
    };
    if raw.contains('\0')
        || raw.contains("..")
        || (raw.contains('/') && raw.matches('/').count() > 1)
    {
        return fail("invalid id");
    }
    let id = qualify_soundpack_id(&raw, "keyboard/");
    let name = id.strip_prefix("keyboard/").unwrap_or(&id).to_string();
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return fail("invalid id");
    }
    // Preinstalled packs come back on next sync (sora-build.sh restores any
    // share pack still present in daemon/soundpacks), so deleting one only
    // looks permanent for a session and then silently reappears. Refuse
    // loudly instead — same union (stamp ∪ allowlist) as packs().
    if folders::soundpacks::bundled_pack_ids().contains(&id) || BUNDLED_PACKS.contains(&id.as_str())
    {
        return fail("cannot delete preinstalled pack");
    }
    let base = folders::soundpacks::get_builtin_soundpacks_dir();
    let target = base.join("keyboard").join(&name);
    if !target.join("config.json").exists() {
        return fail("pack not found");
    }
    // Symlink containment: either side failing to canonicalize denies the
    // delete (fail closed) rather than falling back to the un-resolved path,
    // which would bypass the starts_with check below.
    let canon_base = match base.canonicalize() {
        Ok(p) => p,
        Err(_) => return fail("invalid path"),
    };
    let canon_target = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => return fail("invalid path"),
    };
    if !canon_target.starts_with(&canon_base) {
        return fail("invalid path");
    }
    if let Err(e) = std::fs::remove_dir_all(&target) {
        return fail(&format!("delete failed: {}", e));
    }
    // Hold the cache lock across load → mutate → save so a concurrent
    // engine-worker cache write cannot slip between our load and save.
    let _cache_guard = crate::state::packs::cache_lock();
    let mut cache = crate::state::packs::SoundpackCache::load_locked();
    cache.soundpacks.remove(&id);
    cache.update_count();
    cache.save_locked();

    let was_active = crate::state::settings_saver::current().keyboard_soundpack == id;
    crate::state::settings_saver::apply(|c| {
        c.per_pack_volume.remove(&id);
    });
    if was_active {
        let base2 = folders::soundpacks::get_builtin_soundpacks_dir();
        let ids = collect_packs(&base2, "keyboard");
        let next = if ids.is_empty() {
            String::new()
        } else {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as usize;
            ids[n % ids.len()].clone()
        };
        let rec = if !next.is_empty() {
            recommended_volume_for(&next)
        } else {
            None
        };
        crate::state::settings_saver::apply(|c| {
            c.keyboard_soundpack = next.clone();
            if !next.is_empty() && !c.per_pack_volume.contains_key(&next) {
                if let Some(v) = rec {
                    let v = v.clamp(0.1, 1.0);
                    if (v - 1.0).abs() > 0.001 {
                        c.per_pack_volume.insert(next.clone(), v);
                    }
                }
            }
        });
        let eff = crate::state::settings_saver::current().effective_volume();
        if !next.is_empty() {
            engine.send(AudioCommand::LoadKeyboardPack {
                soundpack_id: next.clone(),
                update_cache_on_error: true,
            });
            engine.send(AudioCommand::SetVolume(eff));
        } else {
            // Last pack deleted: unload so deleted audio stops immediately.
            // Config keyboard_pack stays "" (no sound) for consistency.
            engine.send(AudioCommand::LoadKeyboardPack {
                soundpack_id: String::new(),
                update_cache_on_error: false,
            });
            engine.send(AudioCommand::SetVolume(
                crate::state::settings_saver::current().volume,
            ));
        }
        ok(serde_json::json!({ "deleted": id, "fallback": next }))
    } else {
        ok(serde_json::json!({ "deleted": id }))
    }
}

/// Compile-time fallback for preinstalled ids (used when the
/// sora-build.sh stamp is missing, e.g. first boot before sync).
/// Mirrors daemon/soundpacks/keyboard/* — update both together.
const BUNDLED_PACKS: &[&str] = &[
    "keyboard/aula-f75",
    "keyboard/epomaker-rt85",
    "keyboard/gravastar-v60-pro",
    "keyboard/pmo-aurora-80",
    "keyboard/sugar65",
    "keyboard/yunzii-al65",
];

/// List available packs. `bundled` lists the subset that shipped with the
/// plugin (stamp truth, allowlist fallback) so the UI can badge `(pre)`.
fn packs() -> String {
    let base = folders::soundpacks::get_builtin_soundpacks_dir();
    let mut keyboard: Vec<String> = collect_packs(&base, "keyboard");
    keyboard.sort();
    let stamp = folders::soundpacks::bundled_pack_ids();
    let bundled: Vec<String> = keyboard
        .iter()
        .filter(|id| stamp.contains(id.as_str()) || BUNDLED_PACKS.iter().any(|b| b == id))
        .cloned()
        .collect();
    ok(serde_json::json!({ "keyboard": keyboard, "bundled": bundled }))
}

fn collect_packs(base: &Path, kind: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base.join(kind)) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let id = format!("{kind}/{name}");
            if e.path().join("config.json").exists() {
                ids.push(id);
            }
        }
    }
    ids
}

fn audio_devices() -> String {
    let dm = crate::libs::speakers::DeviceManager::new();
    let devices = match dm.get_output_devices() {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let selected = crate::state::settings_saver::current().selected_audio_device;
    ok(
        serde_json::json!({ "devices": devices.iter().map(|d| serde_json::json!({"id": d.id, "name": d.name, "is_default": d.is_default})).collect::<Vec<_>>(), "selected": selected }),
    )
}

fn select_device(req: &serde_json::Value, engine: &AudioEngineHandle) -> String {
    let id = match req.get("id") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_str() {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            Some(_) => None,
            None => return fail("id must be a string or null"),
        },
    };
    if let Some(ref s) = id {
        if s.contains('\0') || s.len() > 256 {
            return fail("invalid id");
        }
        // Validate against the current enumeration (""/null = system default).
        let known = match crate::libs::speakers::DeviceManager::new().get_output_devices() {
            Ok(devs) => devs.iter().any(|d| &d.id == s) || s == "output_default",
            Err(e) => return fail(&format!("cannot list devices: {e}")),
        };
        if !known {
            return fail("unknown device");
        }
    }
    crate::state::settings_saver::apply(|c| c.selected_audio_device = id.clone());
    if !engine.send(AudioCommand::SwitchDevice(id.clone())) {
        return fail("engine unavailable");
    }
    ok(serde_json::json!({ "selected": id }))
}

fn ok(mut v: serde_json::Value) -> String {
    if let Some(obj) = v.as_object_mut() {
        obj.insert("ok".into(), serde_json::json!(true));
    }
    v.to_string()
}

fn too_many_per_pack_entries(id: &str) -> bool {
    const MAX: usize = 500;
    let c = crate::state::settings_saver::current();
    c.per_pack_volume.len() >= MAX && !c.per_pack_volume.contains_key(id)
}

fn proc_kb(key: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|l| {
            if l.starts_with(key) {
                l.split_whitespace().nth(1)?.parse().ok()
            } else {
                None
            }
        })
}

fn diag() -> String {
    let vm_rss = proc_kb("VmRSS:").unwrap_or(0);
    let vm_hwm = proc_kb("VmHWM:").unwrap_or(0);
    let c = crate::state::settings_saver::current();
    let cache = crate::state::packs::SoundpackCache::load();
    let mut v = serde_json::json!({
        "vm_rss_kb": vm_rss,
        "vm_hwm_kb": vm_hwm,
        "per_pack_volume_entries": c.per_pack_volume.len(),
        "soundpack_cache_entries": cache.soundpacks.len(),
        "keyboard_pack": c.keyboard_soundpack,
    });
    if let Some(obj) = v.as_object_mut() {
        for (k, val) in crate::state::status::snapshot()
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            obj.insert(k, val);
        }
    }
    ok(v)
}

fn export_logs() -> String {
    use chrono::Local;
    let contents = crate::utils::logs::export_contents();
    let name = format!("sorakey-log-{}.txt", Local::now().format("%Y%m%d-%H%M%S"));
    ok(serde_json::json!({
        "name": name,
        "contents": contents,
        "lines": crate::utils::logs::len(),
    }))
}

fn get_bar_section() -> String {
    let home = std::env::var("HOME")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    let path = std::path::PathBuf::from(&home).join(".local/share/sorakey/bar-section");
    if let Ok(s) = std::fs::read_to_string(&path) {
        ok(serde_json::json!({ "section": s.trim() }))
    } else {
        ok(serde_json::json!({ "section": "right" }))
    }
}

fn set_bar_section(req: &serde_json::Value) -> String {
    let Some(section) = req.get("section").and_then(|s| s.as_str()) else {
        return fail("missing section");
    };
    if !matches!(section, "left" | "center" | "right") {
        return fail("invalid section");
    };
    let home = std::env::var("HOME")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    let dir = std::path::PathBuf::from(home).join(".local/share/sorakey");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(&format!("could not create dir: {e}"));
    }
    let path = dir.join("bar-section");
    if let Err(e) = std::fs::write(&path, section) {
        return fail(&format!("could not write: {e}"));
    }
    ok(serde_json::json!({ "section": section }))
}

fn fail(e: &str) -> String {
    serde_json::json!({ "ok": false, "error": e }).to_string()
}

/// `sorakey ctl '<json>'` client — one request, one response line.
pub fn ctl_client(request: &str) -> i32 {
    let path = socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => {
            print!("{}", fail("daemon not running"));
            return 1;
        }
    };
    if stream.write_all(request.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
        print!("{}", fail("write failed"));
        return 1;
    }
    let mut out = String::new();
    {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut out).is_err() {
            print!("{}", fail("read failed"));
            return 1;
        }
    }
    if out.trim().is_empty() {
        print!("{}", fail("empty reply"));
        return 1;
    }
    print!("{out}");
    match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) if v.get("ok") == Some(&serde_json::Value::Bool(true)) => 0,
        Ok(_) => 1,
        Err(_) => 1,
    }
}

/// `sorakey key <Code> [up]` client — fire-and-forget: write one line and
/// close without waiting for the reply (the server ignores write errors).
pub fn key_client(code: &str, down: bool) -> i32 {
    if code.is_empty() || code.len() > 32 || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return 1;
    }
    let path = socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    let request = serde_json::json!({ "cmd": "key", "code": code, "down": down }).to_string();
    if stream.write_all(request.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled allowlist must match daemon/soundpacks/keyboard/* or
    /// the (pre) badge drifts from what actually ships.
    #[test]
    fn bundled_allowlist_matches_shipped_packs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("soundpacks/keyboard");
        let mut shipped: Vec<String> = std::fs::read_dir(&root)
            .expect("soundpacks/keyboard must exist")
            .flatten()
            .filter(|e| e.path().join("config.json").exists())
            .map(|e| format!("keyboard/{}", e.file_name().to_string_lossy()))
            .collect();
        shipped.sort();
        let mut listed: Vec<&str> = BUNDLED_PACKS.to_vec();
        listed.sort_unstable();
        assert_eq!(listed, shipped, "BUNDLED_PACKS drifted from shipped packs");
    }

    /// Oversized requests must be rejected, not buffered to OOM.
    #[test]
    fn oversized_requests_are_rejected_not_buffered() {
        let (server, client) = UnixStream::pair().expect("socketpair");
        let mut client = client;

        let filler: String = "a".repeat(70 * 1024);
        let request = format!("{{\"cmd\":\"status\",\"pad\":\"{filler}\"}}\n");
        client.write_all(request.as_bytes()).expect("client write");

        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        let engine = AudioEngineHandle { tx: cmd_tx };

        let server = server;
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("timeout");
        handle_conn(server, &engine);

        let mut response = String::new();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("timeout");
        client.read_to_string(&mut response).expect("read response");
        assert!(
            response.contains("request too large"),
            "oversized request must be rejected, got: {response}"
        );
    }

    /// Preinstalled packs must be refused before any filesystem touch,
    /// or a delete would look permanent for one session and silently
    /// reappear on next sync.
    #[test]
    fn delete_preinstalled_pack_is_denied() {
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        let engine = AudioEngineHandle { tx: cmd_tx };
        for id in BUNDLED_PACKS {
            let req = serde_json::json!({"id": id});
            let resp = delete_pack(&req, &engine);
            assert!(
                resp.contains("\"ok\":false") && resp.contains("preinstalled"),
                "preinstalled {id} must be denied, got: {resp}"
            );
        }
    }

    /// Normal-sized requests still get a normal response.
    #[test]
    fn normal_requests_are_answered() {
        let (server, client) = UnixStream::pair().expect("socketpair");
        let mut client = client;

        client
            .write_all(b"{\"cmd\":\"status\"}\n")
            .expect("client write");

        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        let engine = AudioEngineHandle { tx: cmd_tx };

        let server = server;
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("timeout");
        handle_conn(server, &engine);

        let mut response = String::new();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("timeout");
        client.read_to_string(&mut response).expect("read response");
        assert!(
            response.contains("\"ok\":true"),
            "status must succeed, got: {response}"
        );
    }
}
