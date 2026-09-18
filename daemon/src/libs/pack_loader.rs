use crate::state::folders;
use crate::state::packs::SoundPack;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::player::{KeySegments, Segment};

/// (samples, channels, sample_rate) for a decoded buffer.
type DecodedAudio = (Arc<Vec<f32>>, u16, u32);

/// A fully decoded soundpack: the precomputed, fade-applied segments at the
/// device rate. `originals` is always
/// empty on a prepared pack: native-rate buffers are freed right after
/// segments are built, and a device switch re-decodes from disk (rare)
/// instead of pinning 10-20 MB forever. `Send` so the worker thread can
/// produce it off the engine thread.
pub(crate) struct LoadedPack {
    pub(super) soundpack: SoundPack,
    /// Native-rate audio for each key. Empty after `prepare_pack_segments`;
    /// only populated transiently between `load_pack` and preparation.
    pub(super) originals: HashMap<String, DecodedAudio>,
    pub(super) segments: HashMap<String, KeySegments>,
}

/// Like `load_audio_file` but takes an explicit file path instead of reading
/// `soundpack.audio_file`. Used for loading per-key audio files in multi-method
/// packs.
fn load_audio_file_for_path(
    sound_file_path: &str,
    device_rate: Option<u32>,
) -> Result<(DecodedAudio, DecodedAudio), String> {
    if !std::path::Path::new(sound_file_path).exists() {
        return Err(format!("Sound file not found: {}", sound_file_path));
    }

    let (samples, channels, file_rate) = load_audio_with_symphonia(sound_file_path)
        .map_err(|e| format!("Failed to load audio: {}", e))?;

    match device_rate {
        Some(device_rate) if device_rate != file_rate => {
            let start = std::time::Instant::now();
            let resampled = super::sound_quality::resample_interleaved(
                &samples,
                channels,
                file_rate,
                device_rate,
            )
            .map_err(|e| format!("Failed to resample audio: {}", e))?;
            crate::always_print!(
                "🔁 Resampled soundpack audio {}Hz -> {}Hz in {:.1}ms (Cubic 64/32 0.95)",
                file_rate,
                device_rate,
                start.elapsed().as_secs_f64() * 1000.0
            );
            let original = (Arc::new(samples), channels, file_rate);
            Ok((original, (Arc::new(resampled), channels, device_rate)))
        }
        _ => {
            let shared = Arc::new(samples);
            Ok((
                (shared.clone(), channels, file_rate),
                (shared, channels, file_rate),
            ))
        }
    }
}

fn load_audio_with_symphonia(file_path: &str) -> Result<(Vec<f32>, u16, u32), String> {
    // Per-file size cap: a corrupt/absurd multi-GB file must be rejected up
    // front instead of decoded into memory (real packs are a few MB; 100MB
    // is already ~40x headroom).
    const MAX_AUDIO_FILE_BYTES: u64 = 100 * 1024 * 1024;
    let meta =
        std::fs::metadata(file_path).map_err(|e| format!("Failed to get file metadata: {}", e))?;
    if meta.len() == 0 {
        return Err(format!("Audio file is empty: {}", file_path));
    }
    if meta.len() > MAX_AUDIO_FILE_BYTES {
        return Err(format!(
            "Audio file too large ({} bytes, cap {} bytes): {}",
            meta.len(),
            MAX_AUDIO_FILE_BYTES,
            file_path
        ));
    }
    crate::utils::sound_reader::decode_interleaved(file_path)
}

/// Convert a V1 pack to V2 in place, with a backup — the one sanctioned
/// write-to-a-pack path. The metadata scan is a pure read (B5), so a pack
/// becomes usable exactly when it is loaded (manually dropped packs; panel
/// imports arrive pre-converted). A backup that cannot be written means the
/// conversion is unrecoverable, so refuse to start it rather than convert
/// without a safety net.
fn convert_v1_if_needed(config_path: &str) -> Result<(), String> {
    use crate::utils::pack_checker::{SoundpackValidationStatus, validate_soundpack_config};
    let validation = validate_soundpack_config(config_path);
    if validation.status != SoundpackValidationStatus::VersionOneNeedsConversion {
        return Ok(());
    }
    if !validation.can_be_converted {
        return Err(format!(
            "V1 pack cannot be converted: {}",
            validation.message
        ));
    }

    let backup_path = numbered_v1_backup_path(config_path);
    std::fs::copy(config_path, &backup_path).map_err(|e| {
        format!(
            "Refusing to convert: could not back up {} to {}: {}",
            config_path, backup_path, e
        )
    })?;

    crate::utils::old_pack_fixer::convert_v1_to_v2(config_path, config_path, None).map_err(|e| {
        if let Err(restore_err) = std::fs::copy(&backup_path, config_path) {
            return format!(
                "Failed to convert {} from V1 to V2: {} (AND restore from {} failed: {}; original may be damaged)",
                config_path, e, backup_path, restore_err
            );
        }
        format!("Failed to convert {} from V1 to V2: {}", config_path, e)
    })
}

/// Backup path for a V1 config: first conversion takes `<config>.v1.backup`,
/// later ones take numbered suffixes, and once the set is full the oldest
/// slot is dropped and re-taken (bounded — the original fixed name meant
/// re-running a conversion overwrote the pristine original with previously
/// generated output).
fn numbered_v1_backup_path(config_path: &str) -> String {
    const MAX_V1_BACKUPS: u32 = 3;
    let first = format!("{}.v1.backup", config_path);
    if !std::path::Path::new(&first).exists() {
        return first;
    }
    for attempt in 1..MAX_V1_BACKUPS {
        let candidate = format!("{}.v1.backup.{}", config_path, attempt);
        if !std::path::Path::new(&candidate).exists() {
            return candidate;
        }
    }
    // ponytail: rotation ceiling — in a repeated-failure loop every backup is
    // an identical copy of the restored original, so the oldest (unnumbered)
    // slot is dropped and re-taken; at most MAX_V1_BACKUPS files ever exist.
    // Upgrade to newest-N shift if distinct V1 configs ever compete for slots.
    let _ = std::fs::remove_file(&first);
    first
}

/// Pure decode of a soundpack's audio — safe to run on any thread. No
/// resampling, no cache writes, no engine state: the result is handed to the
/// engine thread which prepares it at the device's rate.
pub(super) fn load_pack(soundpack_id: &str) -> Result<LoadedPack, String> {
    if soundpack_id.is_empty() {
        return Err("empty soundpack ID".to_string());
    }

    let dir = folders::soundpacks::contained_dir(soundpack_id)
        .ok_or_else(|| format!("Invalid soundpack path: {}", soundpack_id))?;
    let config_path = folders::soundpacks::contained_file(&dir, "config.json")
        .ok_or_else(|| format!("Invalid soundpack config path: {}", soundpack_id))?;
    let soundpack_path = dir.to_string_lossy().to_string();
    convert_v1_if_needed(config_path.to_str().unwrap_or(""))?;
    let config_content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {}", e))?;
    let soundpack: SoundPack = serde_json::from_str(&config_content)
        .map_err(|e| format!("Failed to parse V2 soundpack config: {}", e))?;

    let mut originals: HashMap<String, DecodedAudio> = HashMap::new();
    if soundpack.definition_method == "multi" {
        // Multi-method: decode each unique per-key audio file once.
        let mut file_cache: HashMap<String, DecodedAudio> = HashMap::new();
        let mut failed_files: Vec<String> = Vec::new();
        for (key, key_def) in &soundpack.definitions {
            let audio_file = match &key_def.audio_file {
                Some(f) => f,
                None => continue,
            };
            if let Some(cached) = file_cache.get(audio_file) {
                originals.insert(key.clone(), cached.clone());
                continue;
            }
            let file_path = match folders::soundpacks::contained_file(&dir, audio_file) {
                Some(p) => p.to_string_lossy().to_string(),
                None => {
                    crate::always_eprint!(
                        "⚠️ [Engine] Skipping invalid per-key audio path '{}'",
                        audio_file
                    );
                    failed_files.push(audio_file.clone());
                    continue;
                }
            };
            match load_audio_file_for_path(&file_path, None) {
                Ok((original, _resampled)) => {
                    let (_samples, channels, sample_rate) = &original;
                    crate::always_print!(
                        "✅ [Engine] Loaded multi-method audio file '{}' ({}Hz, {}ch)",
                        audio_file,
                        sample_rate,
                        channels
                    );
                    file_cache.insert(audio_file.clone(), original.clone());
                    originals.insert(key.clone(), original);
                }
                Err(e) => {
                    crate::always_eprint!(
                        "⚠️ [Engine] Failed to load per-key audio '{}': {}",
                        audio_file,
                        e
                    );
                    failed_files.push(audio_file.clone());
                }
            }
        }
        // A half-broken pack used to install with dead keys and no summary:
        // name every failed file once, so the damage is visible in one place.
        if !failed_files.is_empty() {
            failed_files.sort();
            failed_files.dedup();
            crate::always_eprint!(
                "⚠️ [Engine] Pack '{}' loaded with {} dead audio file(s): {}",
                soundpack_id,
                failed_files.len(),
                failed_files.join(", ")
            );
        }
    } else {
        // Single-method: one shared audio file for every key.
        let audio_file = soundpack
            .audio_file
            .as_ref()
            .ok_or_else(|| "No audio_file field in soundpack config".to_string())?;
        let sound_file_path =
            folders::soundpacks::contained_file(Path::new(&soundpack_path), audio_file)
                .ok_or_else(|| format!("Invalid audio_file path: {}", audio_file))?
                .to_string_lossy()
                .to_string();
        let (original, _resampled) = load_audio_file_for_path(&sound_file_path, None)?;
        for key in soundpack.definitions.keys() {
            originals.insert(key.clone(), original.clone());
        }
    }

    // Zero decoded buffers means every file failed (multi-method skips bad
    // per-key files above): report Err so health shows pack_error instead of
    // installing a silent pack with `loaded: true` and no audio.
    if originals.is_empty() {
        return Err(format!(
            "No audio files could be loaded for soundpack '{}'",
            soundpack_id
        ));
    }
    crate::always_print!("✅ [Engine] Decoded keyboard soundpack: {}", soundpack.name);
    Ok(LoadedPack {
        soundpack,
        originals,
        segments: HashMap::new(),
    })
}

/// Decode + resample + precompute in one shot, safe to run on any thread.
/// The engine thread only swaps the result in, so a keystroke is never
/// queued behind a pack load.
pub(super) fn load_pack_prepared(
    soundpack_id: &str,
    device_rate: Option<u32>,
) -> Result<LoadedPack, String> {
    // No output rate means no usable audio device: fail loud so health shows
    // pack_error. Returning the unprepared pack here used to install a pack
    // with empty segments that played nothing while reporting success.
    let device_rate = device_rate.ok_or_else(|| {
        "No output sample rate available (no audio device); cannot prepare soundpack".to_string()
    })?;
    let pack = load_pack(soundpack_id)?;
    let prepared = prepare_pack_segments(pack, device_rate)?;
    // Decode + resample scratch is freed by now; hand the pages back instead
    // of letting this worker thread's arena pin them as RSS. glibc-only;
    // elsewhere this is a no-op foreign call, so skip it entirely.
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
    Ok(prepared)
}

/// Resamples the pack's native-rate audio to `device_rate` and slices +
/// fades the (press, release) segment for every key. Called from the load
/// worker thread (or at startup); takes the pack by value and moves its
/// buffers into the result (no full-buffer clone).
pub(super) fn prepare_pack_segments(
    pack: LoadedPack,
    device_rate: u32,
) -> Result<LoadedPack, String> {
    let mut segments: HashMap<String, KeySegments> = HashMap::with_capacity(pack.originals.len());
    // Resample each unique buffer once: single-method packs share one Arc
    // across every key, multi-method packs one per audio file. Keyed by the
    // buffer's allocation pointer so shared buffers resample exactly once.
    let mut resample_cache: HashMap<*const f32, (Arc<Vec<f32>>, u32)> = HashMap::new();

    for (key, def) in &pack.soundpack.definitions {
        let (samples, channels, file_rate) = match pack.originals.get(key) {
            Some(d) => d,
            None => continue,
        };
        let (base, base_rate) = if *file_rate != device_rate {
            let ptr = samples.as_ptr();
            if let Some((cached, rate)) = resample_cache.get(&ptr) {
                (cached.clone(), *rate)
            } else {
                let resampled = Arc::new(
                    super::sound_quality::resample_interleaved(
                        samples,
                        *channels,
                        *file_rate,
                        device_rate,
                    )
                    .map_err(|e| format!("Failed to resample audio: {}", e))?,
                );
                crate::always_print!(
                    "🔁 Resampled soundpack audio {}Hz -> {}Hz",
                    file_rate,
                    device_rate
                );
                resample_cache.insert(ptr, (resampled.clone(), device_rate));
                (resampled, device_rate)
            }
        } else {
            (samples.clone(), *file_rate)
        };

        let press = def
            .timing
            .first()
            .and_then(|t| build_segment(&base, *channels, base_rate, t[0], t[1]));
        let release = def
            .timing
            .get(1)
            .and_then(|t| build_segment(&base, *channels, base_rate, t[0], t[1]));
        segments.insert(key.clone(), (press, release));
    }

    Ok(LoadedPack {
        soundpack: pack.soundpack,
        // Free the native-rate buffers: segments carry everything playback
        // needs at the device rate. Halves resident memory per pack; a
        // device switch re-decodes from disk (see `EngineState::prepare_pack`).
        originals: HashMap::new(),
        segments,
    })
}

/// Cuts the [start_ms, end_ms) slice out of `base` and pre-applies the fade.
/// Returns `None` for malformed/empty segments (logged at load, not per
/// keypress).
fn build_segment(
    base: &Arc<Vec<f32>>,
    channels: u16,
    sample_rate: u32,
    start_ms: f32,
    end_ms: f32,
) -> Option<Segment> {
    let duration = end_ms - start_ms;
    if start_ms < 0.0 || duration <= 0.0 {
        return None;
    }
    let start_sample = ((start_ms / 1000.0) * (sample_rate as f32) * (channels as f32)) as usize;
    let end_sample = ((end_ms / 1000.0) * (sample_rate as f32) * (channels as f32)) as usize;
    let end_sample = end_sample.min(base.len());
    if start_sample >= base.len() || end_sample <= start_sample {
        crate::always_eprint!(
            "⚠️ [Engine] Dropping invalid segment [start={}ms end={}ms] (buffer {} samples)",
            start_ms,
            end_ms,
            base.len()
        );
        return None;
    }
    let mut segment_samples = base[start_sample..end_sample].to_vec();
    super::player::apply_fade(&mut segment_samples, channels, sample_rate);
    Some((Arc::new(segment_samples), channels, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_backup_path_is_bounded_and_reuses_the_oldest_slot() {
        let dir = std::env::temp_dir().join(format!(
            "sora-v1bk-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let cfg_path = dir.join("config.json");
        std::fs::write(&cfg_path, "{}").unwrap();
        let cfg = cfg_path.to_str().unwrap();
        // Fill the set: first, .1, .2
        let p1 = numbered_v1_backup_path(cfg);
        std::fs::write(&p1, b"a").unwrap();
        let p2 = numbered_v1_backup_path(cfg);
        std::fs::write(&p2, b"b").unwrap();
        let p3 = numbered_v1_backup_path(cfg);
        std::fs::write(&p3, b"c").unwrap();
        assert_ne!(p1, p2);
        assert_ne!(p2, p3);
        // Set full → the oldest (unnumbered) slot is dropped and re-taken;
        // no fourth file appears.
        let p4 = numbered_v1_backup_path(cfg);
        assert_eq!(p4, p1);
        // Caller now writes into the re-taken slot (mirrors
        // convert_v1_if_needed's fs::copy after the path is chosen).
        std::fs::write(&p4, b"d").unwrap();
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 4, "config.json + at most 3 backups");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
