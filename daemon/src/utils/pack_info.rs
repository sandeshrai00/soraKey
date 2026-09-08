use crate::state::folders;
use crate::state::packs::SoundpackMetadata;
use crate::utils::pack_checker::{SoundpackValidationStatus, validate_soundpack_config};
use std::fs;

/// Load soundpack metadata from config.json. Pure read — never writes to the
/// pack. V1→V2 conversion lives in the pack-load path (soundpack_loader), not
/// here: a scan that mutates user files is how B5 destroyed multi packs.
pub fn load_soundpack_metadata(soundpack_id: &str) -> Result<SoundpackMetadata, String> {
    // Refuse symlink escapes before reading anything (scanner-visible error).
    folders::soundpacks::contained_dir(soundpack_id)
        .ok_or_else(|| format!("Soundpack escapes soundpacks dir: {}", soundpack_id))?;
    let config_path = folders::soundpacks::config_json(soundpack_id);

    // Validate the soundpack configuration first
    let validation_result = validate_soundpack_config(&config_path);

    // A pack built for a newer release still yields readable metadata (name,
    // author, icon), so it is listed rather than dropped. Carrying the reason
    // in `last_error` is what keeps "why is this one not working" answerable -
    // the status string alone has no text for a human to read.
    let mut last_error: Option<String> = match validation_result.status {
        SoundpackValidationStatus::RequiresNewerAppVersion(_) => {
            Some(validation_result.message.clone())
        }
        _ => None,
    };

    let content =
        fs::read_to_string(&config_path).map_err(|e| format!("Failed to read config: {}", e))?;

    let config: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))?;

    // Debug: Check if config has audio_file field
    let audio_file = config.get("audio_file").and_then(|v| v.as_str());
    crate::always_print!("🔍 [CACHE DEBUG] soundpack_id: {}", soundpack_id);
    crate::always_print!("🔍 [CACHE DEBUG] config_path: {}", config_path);
    crate::always_print!("🔍 [CACHE DEBUG] audio_file in config: {:?}", audio_file);

    // If audio_file exists, check if the actual file exists
    if let Some(audio_filename) = audio_file {
        let soundpack_dir = folders::soundpacks::soundpack_dir(soundpack_id);
        let full_audio_path = format!(
            "{}/{}",
            soundpack_dir,
            audio_filename.trim_start_matches("./")
        );
        crate::always_print!("🔍 [CACHE DEBUG] soundpack_dir: {}", soundpack_dir);
        crate::always_print!("🔍 [CACHE DEBUG] full_audio_path: {}", full_audio_path);
        crate::always_print!(
            "🔍 [CACHE DEBUG] audio file exists: {}",
            std::path::Path::new(&full_audio_path).exists()
        );

        if !std::path::Path::new(&full_audio_path).exists() {
            crate::always_print!(
                "⚠️ [CACHE DEBUG] Audio file not found during cache refresh: {}",
                full_audio_path
            );
            // Error channel for the panel: a pack whose audio is missing
            // would otherwise list as healthy and then play nothing.
            last_error = Some(format!("audio file not found: {}", full_audio_path));
        }
    } else {
        crate::always_print!("⚠️ [CACHE DEBUG] No audio_file field found in config");
        // Multi-method packs carry per-key files instead of a shared one, so
        // only single-style packs are actually broken by a missing field.
        let has_per_key_audio = config
            .get("definitions")
            .or_else(|| config.get("defs"))
            .and_then(|d| d.as_object())
            .map(|o| o.values().any(|v| v.get("audio_file").is_some()))
            .unwrap_or(false);
        if !has_per_key_audio {
            last_error = Some("config is missing the audio_file field".to_string());
        }
    }

    let name = config
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(soundpack_id)
        .to_string();

    let version = config
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0.0")
        .to_string();

    let tags = config
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Get file stats
    let metadata =
        fs::metadata(&config_path).map_err(|e| format!("Failed to get metadata: {}", e))?;
    Ok(SoundpackMetadata {
        id: soundpack_id.to_string(), // Use soundpack_id (with prefix) instead of config ID
        name,
        author: config
            .get("author")
            .or_else(|| config.get("m_author"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        description: config
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        version,
        tags,
        icon: config
            .get("icon")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        folder_path: soundpack_id.to_string(), // Store the relative path (e.g., "keyboard/Super Paper Mario Talk")
        last_modified: metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        // Validation fields
        config_version: validation_result.config_version,
        is_valid_v2: validation_result.is_valid_v2,
        validation_status: match validation_result.status {
            SoundpackValidationStatus::Valid => "valid".to_string(),
            SoundpackValidationStatus::InvalidVersion => "invalid_version".to_string(),
            SoundpackValidationStatus::InvalidStructure(_) => "invalid_structure".to_string(),
            SoundpackValidationStatus::MissingRequiredFields(_) => "missing_fields".to_string(),
            SoundpackValidationStatus::VersionOneNeedsConversion => {
                "v1_needs_conversion".to_string()
            }
            SoundpackValidationStatus::RequiresNewerAppVersion(_) => {
                "requires_newer_app_version".to_string()
            }
        },
        last_error,
    })
}

#[cfg(test)]
mod tests {
    /// B5: the metadata read path must stay write-free. The old rescan code
    /// called `fs::write` here on a V2-multi pack and silently destroyed its
    /// `method`/`audio_file` fields (the "converted" backup bug), and the V1
    /// auto-conversion rewrote configs from this same function. This is the
    /// smallest check that fails if anyone re-introduces a write to the read
    /// path — no filesystem, no temp dirs, no daemon interference.
    #[test]
    fn the_metadata_read_path_never_writes_to_disk() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/utils/pack_info.rs"
        ))
        .expect("read own source");
        let start = src.find("pub fn load_soundpack_metadata").unwrap();
        let body = &src[start..src.find("#[cfg(test)]").unwrap_or(src.len())];
        for forbidden in [
            "fs::write",
            "fs::create_dir",
            "fs::rename",
            "fs::copy",
            "convert_v1_to_v2",
            "to_string_pretty",
        ] {
            assert!(
                !body.contains(forbidden),
                "load_soundpack_metadata must not write to disk, but it uses `{forbidden}` — \
                 a read path that mutates the pack is the B5 data-loss bug"
            );
        }
    }
}
