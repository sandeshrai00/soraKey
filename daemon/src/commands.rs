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
        Err(_) => std::env::temp_dir().join("sorakey").join("sorakey.sock"),
    }
}

/// Spawn the accept loop. Returns the bound socket path.
pub fn serve(engine: AudioEngineHandle) -> Option<PathBuf> {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        if parent == std::env::temp_dir().join("sorakey") {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::set_permissions(
                parent,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            );
        } else {
            let _ = std::fs::create_dir_all(parent);
        }
    }
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
                    let _ = std::thread::Builder::new().spawn(move || {
                        let _ =
                            stream.set_read_timeout(Some(std::time::Duration::from_millis(100)));
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
        "export_logs" => export_logs(),
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
    if std::fs::symlink_metadata(&target)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return fail("refusing to delete a symlinked pack");
    }
    if let Err(e) = std::fs::remove_dir_all(&target) {
        return fail(&format!("delete failed: {}", e));
    }

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
            });
            engine.send(AudioCommand::SetVolume(eff));
        } else {
            // Last pack deleted: unload so deleted audio stops immediately.
            // Config keyboard_pack stays "" (no sound) for consistency.
            engine.send(AudioCommand::LoadKeyboardPack {
                soundpack_id: String::new(),
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
            // .bak.<ts>.<pid> leftovers from the import move-aside are not
            // packs: listing them pollutes the picker and the delete-fallback
            // can randomly land the active pack on a ghost.
            if name.contains(".bak.") {
                continue;
            }
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
    let path = crate::state::folders::get_writable_data_dir().join("bar-section");
    // migration: old location at ~/.local/share/sorakey/bar-section
    let old = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/sorakey/bar-section"))
        .unwrap_or_else(|_| std::env::temp_dir().join("sorakey/bar-section"));
    let raw = std::fs::read_to_string(&path)
        .or_else(|_| std::fs::read_to_string(&old))
        .unwrap_or_else(|_| "right".to_string());
    let s = raw.trim();
    let section = if !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        && matches!(s, "left" | "center" | "right")
    {
        s
    } else {
        "right"
    };
    ok(serde_json::json!({ "section": section }))
}

fn set_bar_section(req: &serde_json::Value) -> String {
    let Some(section) = req.get("section").and_then(|s| s.as_str()) else {
        return fail("missing section");
    };
    if !matches!(section, "left" | "center" | "right") {
        return fail("invalid section");
    };
    let path = crate::state::folders::get_writable_data_dir().join("bar-section");
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return fail(&format!("could not create dir: {e}"));
        }
    }
    match crate::utils::files::create_sibling_temp(&path) {
        Ok((tmp, mut file)) => {
            use std::io::Write;
            if let Err(e) = file.write_all(section.as_bytes()) {
                let _ = std::fs::remove_file(&tmp);
                return fail(&format!("could not write: {e}"));
            }
            let _ = file.sync_all();
            drop(file);
            if let Err(e) = std::fs::rename(&tmp, &path) {
                let _ = std::fs::remove_file(&tmp);
                return fail(&format!("could not write: {e}"));
            }
        }
        Err(e) => return fail(&format!("could not write: {e}")),
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
