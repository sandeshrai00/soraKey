use crate::state::folders;
use crate::state::packs::SoundPack;
use crate::state::packs::{SoundpackCache, SoundpackMetadata};
use std::collections::HashMap;
use std::sync::Arc;

use super::player::{KeySegments, Segment};

/// (samples, channels, sample_rate) for a decoded buffer.
type DecodedAudio = (Arc<Vec<f32>>, u16, u32);

/// A fully decoded soundpack: the precomputed, fade-applied segments at the
/// device rate (+ pack metadata for cache/errors). `originals` is always
/// empty on a prepared pack: native-rate buffers are freed right after
/// segments are built, and a device switch re-decodes from disk (rare)
/// instead of pinning 10-20 MB forever. `Send` so the worker thread can
/// produce it off the engine thread.
pub(crate) struct LoadedPack {
    pub(super) soundpack: SoundPack,
    pub(super) soundpack_path: String,
    /// Native-rate audio for each key. Empty after `prepare_pack_segments`;
    /// only populated transiently between `load_pack` and preparation.
    pub(super) originals: HashMap<String, DecodedAudio>,
    pub(super) segments: HashMap<String, KeySegments>,
}

/// Loads and decodes a soundpack's audio file, then resamples it to
/// `device_rate` if given. Returns `(original, resampled)`; `original` keeps
/// the file's native rate so a later device switch can re-resample without
/// re-reading from disk.
fn load_audio_file(
    soundpack_path: &str,
    soundpack: &SoundPack,
    device_rate: Option<u32>,
) -> Result<(DecodedAudio, DecodedAudio), String> {
    let audio_file = soundpack
        .audio_file
        .as_ref()
        .ok_or_else(|| "No audio_file field in soundpack config".to_string())?;
    let sanitized = audio_file.trim_start_matches("./").replace('\\', "/");
    if sanitized.contains("..") || sanitized.contains('\0') || sanitized.starts_with('/') {
        return Err(format!("Invalid audio_file path: {}", audio_file));
    }
    let sound_file_path = format!("{}/{}", soundpack_path, sanitized);
    load_audio_file_for_path(&sound_file_path, device_rate)
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
            );
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

/// Derives the `type/name` id the cache is keyed by from a pack's absolute
/// path against the single soundpacks root. Kept separate from the root
/// itself so the path logic can be tested without a filesystem.
fn soundpack_id_from_path(soundpack_path: &str) -> String {
    let roots = [crate::utils::files::get_soundpacks_dir_absolute()];
    relative_soundpack_id(soundpack_path, &roots)
}

/// The id for `soundpack_path` relative to whichever of `roots` contains it.
///
/// Falls back to the last two path components rather than the folder name
/// alone: an id without its `keyboard/`/`mouse/` prefix does not match the key
/// the directory scanner uses, and inserting under it duplicates the pack in
/// the cache and in every list rendered from it.
fn relative_soundpack_id(soundpack_path: &str, roots: &[String]) -> String {
    let path = std::path::Path::new(soundpack_path);

    for root in roots {
        if let Ok(relative) = path.strip_prefix(root) {
            return relative.to_string_lossy().replace('\\', "/");
        }
    }

    let mut tail: Vec<&str> = path
        .components()
        .rev()
        .take(2)
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    tail.reverse();

    if tail.is_empty() {
        "unknown".to_string()
    } else {
        tail.join("/")
    }
}

fn create_soundpack_metadata(
    soundpack_path: &str,
    soundpack: &SoundPack,
) -> Result<SoundpackMetadata, String> {
    // Extract the soundpack ID from the full path
    // e.g., "/path/to/soundpacks/keyboard/Apex by teia" -> "keyboard/Apex by teia"
    //
    // Both roots have to be tried. Packs live under either the bundled
    // directory or the custom one in app data, and the id has to come out as
    // `type/name` for either: the scanner keys the cache that way, so an id
    // that loses its `keyboard/` prefix here inserts a second entry for a pack
    // already in the cache and the UI lists it twice. That is why only
    // imported packs duplicated - bundled ones matched the first root and
    // never reached the fallback.
    let id = soundpack_id_from_path(soundpack_path);

    // Get file metadata
    let last_modified = match std::fs::metadata(soundpack_path) {
        Ok(metadata) => metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        Err(_) => 0,
    };

    Ok(SoundpackMetadata {
        id: id.clone(), // Use calculated relative path ID instead of config ID
        name: soundpack.name.clone(),
        author: soundpack.author.clone(),
        description: soundpack.description.clone(),
        version: soundpack
            .version
            .clone()
            .unwrap_or_else(|| "1.0".to_string()),
        tags: soundpack.tags.clone().unwrap_or_default(),
        icon: soundpack.icon.clone(),
        folder_path: id, // Use the derived folder path for loading
        last_modified,
        // Add validation fields with default values
        config_version: Some(soundpack.config_version_num),
        is_valid_v2: true, // Assume valid since it loaded successfully
        validation_status: "valid".to_string(),
        // Error tracking - None since we successfully created metadata
        last_error: None,
    })
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
        let _ = std::fs::copy(&backup_path, config_path); // restore the original
        format!("Failed to convert {} from V1 to V2: {}", config_path, e)
    })
}

/// Backup path for a V1 config that never clobbers a previous run: the first
/// conversion takes `<config>.v1.backup`, later ones take numbered suffixes.
/// The old fixed name meant re-running a conversion overwrote the pristine
/// original with previously generated output.
fn numbered_v1_backup_path(config_path: &str) -> String {
    let first = format!("{}.v1.backup", config_path);
    if !std::path::Path::new(&first).exists() {
        return first;
    }
    let mut attempt: u32 = 1;
    loop {
        let candidate = format!("{}.v1.backup.{}", config_path, attempt);
        if !std::path::Path::new(&candidate).exists() {
            return candidate;
        }
        attempt += 1;
        if attempt > 1000 {
            // Pathological backup pile-up: timestamp instead of looping forever.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            return format!("{}.v1.backup.{}", config_path, nanos);
        }
    }
}

/// Pure decode of a soundpack's audio — safe to run on any thread. No
/// resampling, no cache writes, no engine state: the result is handed to the
/// engine thread which prepares it at the device's rate.
pub(super) fn load_pack(soundpack_id: &str) -> Result<LoadedPack, String> {
    if soundpack_id.is_empty() {
        return Err("empty soundpack ID".to_string());
    }

    let soundpack_path = folders::soundpacks::soundpack_dir(soundpack_id);
    let config_path = folders::soundpacks::config_json(soundpack_id);
    convert_v1_if_needed(&config_path)?;
    let config_content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {}", e))?;
    let soundpack: SoundPack = serde_json::from_str(&config_content)
        .map_err(|e| format!("Failed to parse V2 soundpack config: {}", e))?;

    let mut originals: HashMap<String, DecodedAudio> = HashMap::new();
    if soundpack.definition_method == "multi" {
        // Multi-method: decode each unique per-key audio file once.
        let mut file_cache: HashMap<String, DecodedAudio> = HashMap::new();
        for (key, key_def) in &soundpack.definitions {
            let audio_file = match &key_def.audio_file {
                Some(f) => f,
                None => continue,
            };
            if let Some(cached) = file_cache.get(audio_file) {
                originals.insert(key.clone(), cached.clone());
                continue;
            }
            let sanitized = audio_file.trim_start_matches("./").replace('\\', "/");
            if sanitized.contains("..") || sanitized.contains('\0') || sanitized.starts_with('/') {
                crate::always_eprint!(
                    "⚠️ [Engine] Skipping invalid per-key audio path '{}'",
                    audio_file
                );
                continue;
            }
            let file_path = format!("{}/{}", soundpack_path, sanitized);
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
                }
            }
        }
    } else {
        // Single-method: one shared audio file for every key.
        let (original, _resampled) = load_audio_file(&soundpack_path, &soundpack, None)?;
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
        soundpack_path,
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
    let prepared = prepare_pack_segments(pack, device_rate);
    // Decode + resample scratch is freed by now; hand the pages back instead
    // of letting this worker thread's arena pin them as RSS.
    unsafe {
        libc::malloc_trim(0);
    }
    Ok(prepared)
}

/// Resamples the pack's native-rate audio to `device_rate` and slices +
/// fades the (press, release) segment for every key. Called from the load
/// worker thread (or at startup); takes the pack by value and moves its
/// buffers into the result (no full-buffer clone).
pub(super) fn prepare_pack_segments(pack: LoadedPack, device_rate: u32) -> LoadedPack {
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
                let resampled = Arc::new(super::sound_quality::resample_interleaved(
                    samples,
                    *channels,
                    *file_rate,
                    device_rate,
                ));
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

    LoadedPack {
        soundpack: pack.soundpack,
        soundpack_path: pack.soundpack_path,
        // Free the native-rate buffers: segments carry everything playback
        // needs at the device rate. Halves resident memory per pack; a
        // device switch re-decodes from disk (see `EngineState::prepare_pack`).
        originals: HashMap::new(),
        segments,
    }
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

/// Update the soundpack cache after a successful load.
pub(super) fn update_soundpack_cache(pack: &LoadedPack, soundpack_id: &str) {
    let _guard = crate::state::packs::cache_lock();
    let mut cache = SoundpackCache::load_locked();
    match create_soundpack_metadata(&pack.soundpack_path, &pack.soundpack) {
        Ok(metadata) => {
            cache.add_soundpack(metadata);
        }
        Err(e) => {
            crate::always_print!("⚠️ Failed to create metadata for {}: {}", soundpack_id, e);
        }
    }
    cache.save_locked();
}

/// Capture soundpack loading error and update the cache
pub(super) fn capture_soundpack_loading_error(soundpack_id: &str, error: &str) {
    // Skip creating cache entries for empty soundpack IDs
    if soundpack_id.is_empty() {
        crate::always_print!("⚠️ Skipping cache entry for empty soundpack ID: {}", error);
        return;
    }

    crate::always_print!("📝 Capturing loading error for {}: {}", soundpack_id, error);

    let _guard = crate::state::packs::cache_lock();
    let mut cache = SoundpackCache::load_locked();

    // Check if we already have metadata for this soundpack
    if let Some(existing_metadata) = cache.soundpacks.get_mut(soundpack_id) {
        // Update existing metadata with error
        existing_metadata.last_error = Some(error.to_string());
        existing_metadata.validation_status = "loading_error".to_string();
    } else {
        // Create minimal metadata entry with error information
        let error_metadata = SoundpackMetadata {
            id: soundpack_id.to_string(),
            name: format!("Error: {}", soundpack_id),
            author: None,
            description: Some(format!("Loading failed: {}", error)),
            version: "unknown".to_string(),
            tags: vec!["error".to_string()],
            icon: None,
            folder_path: soundpack_id.to_string(), // Add folder_path for error entries
            last_modified: 0,
            config_version: None,
            is_valid_v2: false,
            validation_status: "loading_error".to_string(),
            last_error: Some(error.to_string()),
        };

        const MAX_CACHE_ENTRIES: usize = 1000;
        if cache.soundpacks.len() >= MAX_CACHE_ENTRIES {
            if let Some(old) = cache
                .soundpacks
                .iter()
                .find(|(_, m)| m.last_error.is_some())
                .map(|(k, _)| k.clone())
            {
                cache.soundpacks.remove(&old);
            } else {
                crate::always_eprint!(
                    "⚠️ cache full ({}), skipping error entry for {}",
                    cache.soundpacks.len(),
                    soundpack_id
                );
                return;
            }
        }
        cache
            .soundpacks
            .insert(soundpack_id.to_string(), error_metadata);
    }

    cache.save_locked();
    crate::always_print!(
        "💾 Updated cache with error information for {}",
        soundpack_id
    );
}

#[cfg(test)]
mod tests {
    use super::relative_soundpack_id;

    /// Joins with the running platform's separator. `Path::components` only
    /// splits on the native one, so a hard-coded `\` is a single component on
    /// Linux and the assertions below would be testing nothing there.
    fn native_path(parts: &[&str]) -> String {
        parts.join(std::path::MAIN_SEPARATOR_STR)
    }

    fn builtin_root() -> String {
        native_path(&["", "opt", "Sorakey", "soundpacks"])
    }

    fn roots() -> Vec<String> {
        vec![builtin_root()]
    }

    #[test]
    fn an_imported_pack_gets_the_same_id_the_scanner_uses() {
        // Only one root now; the scanner keys the cache by `keyboard/name`.
        assert_eq!(
            relative_soundpack_id(
                &native_path(&[&builtin_root(), "keyboard", "eg-oreo"]),
                &roots()
            ),
            "keyboard/eg-oreo"
        );
    }

    #[test]
    fn a_path_under_no_known_root_still_keeps_its_type_prefix() {
        // The fallback keeps two components rather than one, so even an
        // unexpected location cannot produce a prefix-less id.
        assert_eq!(
            relative_soundpack_id(
                &native_path(&["", "elsewhere", "keyboard", "Model O"]),
                &roots()
            ),
            "keyboard/Model O"
        );
    }
}
