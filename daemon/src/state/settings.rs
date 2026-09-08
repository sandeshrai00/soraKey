use crate::state::folders;
use crate::utils::{files, json_files};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Each field defaults on its own — one bad entry doesn't wipe the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    // Metadata
    pub version: String,
    pub last_updated: DateTime<Utc>,
    pub commit: Option<String>,
    // Audio settings
    pub keyboard_soundpack: String,
    pub volume: f32,
    pub enable_sound: bool,
    // Per-pack volume overrides (0.0-1.0 multiplier, 1.0 = pack's recommended_volume)
    #[serde(default)]
    pub per_pack_volume: HashMap<String, f32>,
    // Device settings
    pub selected_audio_device: Option<String>,
    // System settings
    pub auto_start: bool,
}

/// Keep the usable entries of a `per_pack_volume` table, dropping the bad
/// ones. Returns None when the value is not an object at all (nothing to
/// salvage) or when no entry survives the probe.
fn salvage_per_pack_volume(entry: &serde_json::Value) -> Option<serde_json::Value> {
    let object = entry.as_object()?;
    let mut cleaned = serde_json::Map::new();
    for (pack_id, volume) in object {
        let ok = volume.as_f64().is_some_and(|v| {
            let f = v as f32;
            f.is_finite()
        }) && serde_json::from_value::<f32>(volume.clone()).is_ok();
        if ok {
            cleaned.insert(pack_id.clone(), volume.clone());
        } else {
            crate::always_eprint!(
                "⚠️  Ignoring unusable per-pack volume for '{}', keeping the rest",
                pack_id
            );
        }
    }
    let mut probe = serde_json::Map::new();
    probe.insert(
        "per_pack_volume".to_string(),
        serde_json::Value::Object(cleaned.clone()),
    );
    if serde_json::from_value::<AppConfig>(serde_json::Value::Object(probe)).is_ok() {
        Some(serde_json::Value::Object(cleaned))
    } else {
        None
    }
}

/// Parse config leniently — drop bad fields, keep the rest.
pub fn parse_lenient(contents: &str) -> Result<AppConfig, String> {
    let value: serde_json::Value =
        serde_json::from_str(contents).map_err(|e| format!("not valid JSON: {}", e))?;

    let Some(object) = value.as_object() else {
        return Err("config is not a JSON object".to_string());
    };

    // try each key against a default document
    let mut accepted = serde_json::Map::new();
    for (key, entry) in object {
        // null soundpack means "no sound", not an error
        let entry = if entry.is_null() && key.as_str() == "keyboard_soundpack" {
            serde_json::Value::String(String::new())
        } else {
            entry.clone()
        };

        let mut probe = serde_json::Map::new();
        probe.insert(key.clone(), entry.clone());

        if serde_json::from_value::<AppConfig>(serde_json::Value::Object(probe)).is_ok() {
            accepted.insert(key.clone(), entry);
        } else if key.as_str() == "per_pack_volume" {
            // One bad per-pack entry must not drop the whole table:
            // keep the entries that parse as f32, skip the rest.
            if let Some(cleaned) = salvage_per_pack_volume(&entry) {
                accepted.insert(key.clone(), cleaned);
            } else {
                crate::always_eprint!(
                    "⚠️  Ignoring unusable config entry '{}', using its default",
                    key
                );
            }
        } else {
            crate::always_eprint!(
                "⚠️  Ignoring unusable config entry '{}', using its default",
                key
            );
        }
    }

    serde_json::from_value(serde_json::Value::Object(accepted))
        .map_err(|e| format!("config could not be rebuilt: {}", e))
}

/// Result of trying to preserve a corrupt config.
enum Preserved {
    /// There was no file to preserve - a first run, or it was deleted.
    Nothing,
    /// The user's bytes now live at this path and are safe to overwrite from.
    MovedTo(std::path::PathBuf),
    /// The file is still in place and must not be written over.
    Failed(String),
}

/// Move corrupt config aside so user data survives.
fn preserve_corrupt_config(config_path: &std::path::Path) -> Preserved {
    if !config_path.exists() {
        return Preserved::Nothing;
    }

    let base = {
        let mut name = config_path.as_os_str().to_os_string();
        name.push(".corrupt");
        std::path::PathBuf::from(name)
    };

    // numbered suffix so first rescue isn't overwritten
    let mut target = base.clone();
    let mut attempt = 1;
    while target.exists() {
        let mut name = base.as_os_str().to_os_string();
        name.push(format!(".{}", attempt));
        target = std::path::PathBuf::from(name);
        attempt += 1;

        // avoid infinite loop
        if attempt > 100 {
            return Preserved::Failed(
                "too many saved copies of a damaged config already exist".to_string(),
            );
        }
    }

    match std::fs::rename(config_path, &target) {
        Ok(()) => Preserved::MovedTo(target),
        Err(e) => Preserved::Failed(e.to_string()),
    }
}

impl AppConfig {
    /// Has data changed (ignoring metadata)?
    pub fn data_equals(&self, other: &Self) -> bool {
        self.keyboard_soundpack == other.keyboard_soundpack
            && self.volume == other.volume
            && self.enable_sound == other.enable_sound
            && self.selected_audio_device == other.selected_audio_device
            && self.auto_start == other.auto_start
            && self.per_pack_volume == other.per_pack_volume
    }

    /// Effective volume for the current pack.
    pub fn effective_volume(&self) -> f32 {
        if let Some(per) = self.per_pack_volume.get(&self.keyboard_soundpack) {
            per.clamp(0.0, 1.0)
        } else {
            self.volume.clamp(0.0, 1.0)
        }
    }

    pub fn load() -> Self {
        let config_path = folders::data::config_json();

        if let Some(parent) = config_path.parent() {
            if files::ensure_directory_exists(parent).is_err() {
                crate::always_eprint!("⚠️  Could not create data directory");
            }
        }

        crate::always_print!("📖 Loading config from: {}", config_path.display());

        // read then parse — different errors, different handling
        let parsed = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("could not read '{}': {}", config_path.display(), e))
            .and_then(|contents| parse_lenient(&contents));

        match parsed {
            Ok(mut config) => {
                let mut config_updated = false;

                // No pack-ID migrations: removed packs are handled by the
                // engine boot fallback (first available pack on disk).

                // drop index-based device IDs — they're unstable, revert to default

                if let Some(device_id) = config.selected_audio_device.clone() {
                    if crate::libs::speakers::is_legacy_index_device_id(&device_id) {
                        crate::always_print!(
                            "🔄 Dropping index-based audio device {}: reverting to system default",
                            device_id
                        );
                        config.selected_audio_device = None;
                        config_updated = true;
                    }
                }

                // NOTE: no volume 1.0 -> 0.6 migration. 1.0 is a valid user
                // value; rewriting it on every downgrade clobbers an
                // explicit max, so the least-destructive option is to keep
                // whatever the user has.

                // sync auto_start with system, but never let a failed
                // detection overwrite the user's value (non-systemd boxes
                // would otherwise force true -> false on every start).
                match crate::utils::auto_start::get_auto_startup_state() {
                    Some(actual_auto_start) if config.auto_start != actual_auto_start => {
                        crate::always_print!(
                            "🔄 Syncing auto_start config with registry: {} -> {}",
                            config.auto_start,
                            actual_auto_start
                        );
                        config.auto_start = actual_auto_start;
                        config_updated = true;
                    }
                    None => {
                        crate::always_print!(
                            "🔄 Skipping auto_start sync: system state unknown, keeping {}",
                            config.auto_start
                        );
                    }
                    _ => {}
                }

                // save if migrated
                if config_updated {
                    config.last_updated = chrono::Utc::now();
                    let _ = config.save();
                }

                config
            }
            Err(e) => {
                // here means the whole document is unreadable
                crate::always_eprint!("❌ Failed to load config file: {}", e);
                crate::always_eprint!("   Config path: {}", config_path.display());

                // don't overwrite corrupt file — move it aside first
                match preserve_corrupt_config(&config_path) {
                    Preserved::Nothing => {
                        // first run, nothing to preserve
                        let default_config = Self::default();
                        let _ = default_config.save();
                        default_config
                    }
                    Preserved::MovedTo(backup) => {
                        crate::always_eprint!(
                            "   Your previous config was kept at: {}",
                            backup.display()
                        );
                        crate::always_eprint!("   Defaults are in use for this session.");
                        let default_config = Self::default();
                        let _ = default_config.save();
                        default_config
                    }
                    Preserved::Failed(err) => {
                        crate::always_eprint!("   Could not set the damaged config aside: {}", err);
                        crate::always_eprint!(
                            "   Leaving it untouched and running on defaults; \
                             settings changed now will not be saved over it."
                        );
                        Self::default()
                    }
                }
            }
        }
    }

    /// Write config to disk — only via `config_writer::apply` outside this module.
    pub(in crate::state) fn save(&self) -> Result<(), String> {
        let config_path = folders::data::config_json();
        crate::always_print!("💾 Saving config to: {}", config_path.display());
        crate::always_print!("   keyboard_soundpack: {}", self.keyboard_soundpack);
        json_files::save_json_to_file_atomically(self, &config_path)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: crate::utils::version::APP_VERSION.to_string(),
            last_updated: Utc::now(),
            commit: option_env!("GIT_HASH").map(|s| s.to_string()),
            keyboard_soundpack: "keyboard/sugar65".to_string(),
            volume: 0.6,
            enable_sound: true,
            per_pack_volume: HashMap::new(),
            selected_audio_device: None,
            auto_start: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stale save reverts concurrent changes.
    #[test]
    fn holding_a_struct_and_writing_it_back_is_what_reverts_a_concurrent_change() {
        let on_disk = AppConfig::default();

        // volume path reads config
        let mut volume_writer_copy = on_disk.clone();

        // user mutes
        let mut after_mute = on_disk.clone();
        after_mute.enable_sound = false;

        // volume path saves stale copy
        volume_writer_copy.volume = 0.5;
        let persisted = volume_writer_copy;

        assert!(!after_mute.enable_sound, "mute was applied to disk");
        assert!(
            persisted.enable_sound,
            "stale save silently restores enable_sound - which is why no public \
             API accepts a config to write"
        );
    }

    /// Reload before mutate to keep concurrent changes.
    #[test]
    fn reloading_before_mutating_preserves_a_concurrent_change() {
        let on_disk = AppConfig {
            enable_sound: false,
            ..Default::default()
        };

        let mut fresh = on_disk.clone();
        fresh.volume = 0.5;

        assert!(
            !fresh.enable_sound,
            "mute must survive an unrelated config write"
        );
        assert_eq!(fresh.volume, 0.5, "the volume change must still apply");
    }

    /// Helper: build config JSON with edits.
    fn config_json_with(edits: &[(&str, serde_json::Value)]) -> String {
        let mut value = serde_json::to_value(AppConfig::default()).expect("config serializes");
        let object = value.as_object_mut().expect("config is a json object");
        for (key, entry) in edits {
            object.insert((*key).to_string(), entry.clone());
        }
        value.to_string()
    }

    /// Null string field should only affect that field.
    #[test]
    fn a_null_soundpack_costs_only_that_field() {
        let document = config_json_with(&[
            ("keyboard_soundpack", serde_json::Value::Null),
            ("volume", serde_json::json!(0.42)),
            ("auto_start", serde_json::json!(true)),
        ]);

        assert!(
            serde_json::from_str::<AppConfig>(&document).is_err(),
            "a null in a non-optional field must really break a strict parse, \
             or this test is not exercising the bug"
        );

        let restored = parse_lenient(&document).expect("a null field must not fail the document");

        assert_eq!(
            restored.keyboard_soundpack, "",
            "null must read as the no-pack state"
        );
        assert_eq!(
            restored.volume, 0.42,
            "every other setting must survive intact"
        );
        assert!(restored.auto_start, "and so must the rest");
    }

    /// Wrong types and unknown keys only affect their field.
    #[test]
    fn wrong_typed_and_unknown_fields_do_not_fail_the_document() {
        let document = config_json_with(&[
            ("volume", serde_json::json!("loud")),
            ("auto_start", serde_json::json!(true)),
            ("a_key_from_a_future_build", serde_json::json!({ "x": 1 })),
        ]);

        let restored =
            parse_lenient(&document).expect("wrong-typed fields must not fail the document");

        assert_eq!(
            restored.volume,
            AppConfig::default().volume,
            "a damaged volume must fall back to the audible config default, not 0.0"
        );
        assert!(
            restored.auto_start,
            "a valid neighbouring setting must survive"
        );
    }

    /// Missing field defaults without losing the rest.
    #[test]
    fn a_missing_field_defaults_without_losing_the_rest() {
        let mut value = serde_json::to_value(AppConfig::default()).expect("config serializes");
        let object = value.as_object_mut().expect("config is a json object");
        object.remove("auto_start");
        object.insert("volume".to_string(), serde_json::json!(0.8));

        let restored = parse_lenient(&value.to_string())
            .expect("a missing field must default rather than fail");

        assert!(!restored.auto_start, "the absent field takes its default");
        assert_eq!(
            restored.volume, 0.8,
            "the user's other settings must be untouched"
        );
    }

    /// Non-object document is a parse failure.
    #[test]
    fn a_non_object_document_is_a_parse_failure() {
        assert!(parse_lenient("[]").is_err(), "a bare array is not a config");
        assert!(
            parse_lenient("\"hello\"").is_err(),
            "a bare string is not a config"
        );
        assert!(
            parse_lenient("{\"volume\": 0.5").is_err(),
            "a truncated document must fail"
        );
    }

    /// Truncated config is preserved, not overwritten.
    #[test]
    fn a_truncated_config_is_preserved_rather_than_overwritten() {
        let dir = std::env::temp_dir().join(format!("sorakey-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.json");

        let original = br#"{"volume": 0.7, "theme": "#.to_vec();
        std::fs::write(&config_path, &original).expect("write truncated config");

        assert!(
            serde_json::from_slice::<AppConfig>(&original).is_err(),
            "this fixture must really be unparseable, or the test proves nothing"
        );

        let preserved = preserve_corrupt_config(&config_path);
        let backup = match preserved {
            Preserved::MovedTo(path) => path,
            _ => panic!("an existing damaged config must be preserved"),
        };

        assert_eq!(
            std::fs::read(&backup).expect("backup readable"),
            original,
            "the user's original bytes must survive verbatim"
        );
        assert!(
            !config_path.exists(),
            "the damaged file is moved, not copied"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Second failure doesn't clobber first rescue.
    #[test]
    fn a_second_failure_does_not_clobber_the_first_rescue() {
        let dir =
            std::env::temp_dir().join(format!("sorakey-corrupt-twice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.json");

        std::fs::write(&config_path, b"first damaged config").expect("write");
        let first = match preserve_corrupt_config(&config_path) {
            Preserved::MovedTo(path) => path,
            _ => panic!("first preservation must succeed"),
        };

        std::fs::write(&config_path, b"second damaged config").expect("write");
        let second = match preserve_corrupt_config(&config_path) {
            Preserved::MovedTo(path) => path,
            _ => panic!("second preservation must succeed"),
        };

        assert_ne!(first, second, "the second rescue must take a distinct path");
        assert_eq!(
            std::fs::read(&first).expect("first backup readable"),
            b"first damaged config",
            "the earlier rescue must still hold its original bytes"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Missing file needs no rescue.
    #[test]
    fn a_missing_config_file_is_not_treated_as_a_rescue() {
        let dir = std::env::temp_dir().join(format!("sorakey-absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let preserved = preserve_corrupt_config(&dir.join("config.json"));
        assert!(
            matches!(preserved, Preserved::Nothing),
            "an absent config is a first run, not a rescue"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Reasserting same value is not a change.
    #[test]
    fn reasserting_an_identical_value_is_not_a_change() {
        let config = AppConfig::default();
        let mut same = config.clone();
        same.volume = config.volume;

        assert!(
            same.data_equals(&config),
            "no-op write must not be treated as a change"
        );

        same.volume = config.volume + 0.25;
        assert!(
            !same.data_equals(&config),
            "a real change must still be detected"
        );
    }

    /// Index-based device ID is legacy.
    #[test]
    fn an_index_based_device_id_is_recognised_as_legacy_and_droppable() {
        use crate::libs::speakers::is_legacy_index_device_id;

        assert!(is_legacy_index_device_id("output_2"));
        assert!(is_legacy_index_device_id("output_0"));

        // stable IDs survive
        assert!(!is_legacy_index_device_id("output_name:0b3a976597860826"));
        assert!(!is_legacy_index_device_id("output_default"));
    }
}
