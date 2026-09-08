use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum SoundpackValidationStatus {
    Valid,
    InvalidVersion,
    InvalidStructure(String),
    MissingRequiredFields(Vec<String>),
    VersionOneNeedsConversion,
    /// Pack needs a newer app to read it.
    RequiresNewerAppVersion(u32),
}

/// Read `config_version` whether it's a number or a string.
fn read_config_version(config: &Value) -> Option<u32> {
    let raw = config.get("config_version")?;

    if let Some(n) = raw.as_u64() {
        return u32::try_from(n).ok();
    }

    raw.as_str()?.trim().parse::<u32>().ok()
}

/// Key definitions under `definitions` or `defs` (alias).
fn key_definitions(config: &Value) -> Option<&Value> {
    config.get("definitions").or_else(|| config.get("defs"))
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SoundpackValidationResult {
    pub status: SoundpackValidationStatus,
    pub config_version: Option<u32>,
    pub detected_version: Option<String>,
    pub is_valid_v2: bool,
    pub can_be_converted: bool,
    pub message: String,
}

/// Validate soundpack config at path.
pub fn validate_soundpack_config(config_path: &str) -> SoundpackValidationResult {
    let content = match crate::utils::files::read_file_contents(config_path) {
        Ok(content) => content,
        Err(e) => {
            return SoundpackValidationResult {
                status: SoundpackValidationStatus::InvalidStructure(format!(
                    "Cannot read config file: {}",
                    e
                )),
                config_version: None,
                detected_version: None,
                is_valid_v2: false,
                can_be_converted: false,
                message: format!("Failed to read config file: {}", e),
            };
        }
    };

    let config: Value = match serde_json::from_str(&content) {
        Ok(config) => config,
        Err(e) => {
            return SoundpackValidationResult {
                status: SoundpackValidationStatus::InvalidStructure(format!("Invalid JSON: {}", e)),
                config_version: None,
                detected_version: None,
                is_valid_v2: false,
                can_be_converted: false,
                message: format!("Invalid JSON format: {}", e),
            };
        }
    };

    let config_version = read_config_version(&config);
    let package_version = config
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let has_defines = config.get("defines").is_some();
    let has_sound_field = config.get("sound").is_some();
    let has_method_field =
        config.get("method").is_some() || config.get("key_define_type").is_some();

    let has_defs = key_definitions(&config).is_some();
    let _has_source_field = config.get("source").is_some();
    // `m_author` is the accepted author alias (see validate_v2_structure).
    let has_author = config.get("author").is_some() || config.get("m_author").is_some();

    if let Some(version) = config_version.filter(|v| *v > 2) {
        // newer than we understand — ask to update
        SoundpackValidationResult {
            status: SoundpackValidationStatus::RequiresNewerAppVersion(version),
            config_version: Some(version),
            detected_version: package_version,
            is_valid_v2: false,
            can_be_converted: false,
            message: "This soundpack requires a newer version of Sorakey".to_string(),
        }
    } else if config_version == Some(2) {
        // explicit V2
        validate_v2_structure(&config, config_version, package_version)
    } else if config_version == Some(1) || (has_defines && has_sound_field && !has_defs) {
        // explicit V1 or V1-shaped
        SoundpackValidationResult {
            status: SoundpackValidationStatus::VersionOneNeedsConversion,
            config_version: Some(1),
            detected_version: package_version,
            is_valid_v2: false,
            can_be_converted: true,
            message: if has_method_field {
                "Version 1 soundpack with method field detected, needs conversion to V2 format"
                    .to_string()
            } else {
                "Version 1 soundpack detected, needs conversion to V2 format".to_string()
            },
        }
    } else if has_defs && has_author {
        // V2-shaped but unversioned
        validate_v2_structure(&config, None, package_version)
    } else {
        // unknown shape
        let mut missing_fields = Vec::new();

        if !has_defs && !has_defines {
            missing_fields.push("definitions or defines".to_string());
        }

        if config.get("name").is_none() {
            missing_fields.push("name".to_string());
        }

        // Without this a {name + definitions} pack with no author fell
        // through with an empty missing list and a "Missing required
        // fields: " message naming nothing.
        if !has_author {
            missing_fields.push("author".to_string());
        }
        SoundpackValidationResult {
            status: SoundpackValidationStatus::MissingRequiredFields(missing_fields.clone()),
            config_version,
            detected_version: package_version,
            is_valid_v2: false,
            can_be_converted: has_defines && has_sound_field, // Can convert if it looks like V1
            message: format!("Missing required fields: {}", missing_fields.join(", ")),
        }
    }
}

/// Validate V2 soundpack structure
fn validate_v2_structure(
    config: &Value,
    config_version: Option<u32>,
    package_version: Option<String>,
) -> SoundpackValidationResult {
    let mut missing_fields = Vec::new();
    let mut issues = Vec::new();

    // required V2 fields
    if config.get("name").is_none() {
        missing_fields.push("name".to_string());
    }

    if config.get("author").is_none() && config.get("m_author").is_none() {
        missing_fields.push("author".to_string());
    }

    let definitions = key_definitions(config);

    if definitions.is_none() {
        missing_fields.push("definitions".to_string());
    }

    // Single-method packs play one shared file: without it there is no audio
    // to decode (the loader errors). Only the explicit "single" method is
    // checked — unversioned/legacy shapes without a method stay exempt so
    // previously-valid packs keep validating.
    let has_per_key_audio = definitions
        .and_then(|d| d.as_object())
        .map(|o| o.values().any(|v| v.get("audio_file").is_some()))
        .unwrap_or(false);
    if config.get("definition_method").and_then(|m| m.as_str()) == Some("single")
        && !has_per_key_audio
        && config.get("audio_file").is_none()
    {
        issues.push("single-method soundpack is missing the audio_file field".to_string());
    }

    // validate definitions
    if let Some(defs) = definitions {
        if let Some(defs_obj) = defs.as_object() {
            if defs_obj.is_empty() {
                issues.push("definitions must not be empty".to_string());
            }
            for (key, value) in defs_obj {
                // supports both `{ timing: [...] }` and bare `[[...]]`
                let timings = match value.get("timing") {
                    Some(timing) => timing,
                    None => value,
                };

                let arr = match timings.as_array() {
                    Some(arr) => arr,
                    None => {
                        issues.push(format!(
                            "Invalid definitions entry for '{}': expected timing array",
                            key
                        ));
                        continue;
                    }
                };

                for (i, timing) in arr.iter().enumerate() {
                    if let Some(timing_arr) = timing.as_array() {
                        if timing_arr.len() != 2 {
                            issues.push(format!(
                                "Invalid timing array for '{}[{}]': expected [start, end]",
                                key, i
                            ));
                        } else {
                            // Timings must be finite numbers with start < end:
                            // strings/bools/null, 1e999-style infinities, and
                            // reversed ranges all decode to silence or panics
                            // downstream, so reject them here.
                            match (timing_arr[0].as_f64(), timing_arr[1].as_f64()) {
                                (Some(start), Some(end))
                                    if start.is_finite() && end.is_finite() =>
                                {
                                    if start >= end {
                                        issues.push(format!(
                                            "Invalid timing for '{}[{}]': start ({}) must be before end ({})",
                                            key, i, start, end
                                        ));
                                    }
                                }
                                _ => {
                                    issues.push(format!(
                                        "Invalid timing for '{}[{}]': expected [start, end] as finite numbers",
                                        key, i
                                    ));
                                }
                            }
                        }
                    } else {
                        issues.push(format!(
                            "Invalid timing entry for '{}[{}]': expected array",
                            key, i
                        ));
                    }
                }
            }
        } else {
            issues.push("definitions field should be an object".to_string());
        }
    }

    // final status
    if !missing_fields.is_empty() {
        SoundpackValidationResult {
            status: SoundpackValidationStatus::MissingRequiredFields(missing_fields.clone()),
            config_version,
            detected_version: package_version,
            is_valid_v2: false,
            can_be_converted: false,
            message: format!("Missing required V2 fields: {}", missing_fields.join(", ")),
        }
    } else if !issues.is_empty() {
        SoundpackValidationResult {
            status: SoundpackValidationStatus::InvalidStructure(issues.join("; ")),
            config_version,
            detected_version: package_version,
            is_valid_v2: false,
            can_be_converted: false,
            message: format!("V2 structure issues: {}", issues.join("; ")),
        }
    } else {
        SoundpackValidationResult {
            status: SoundpackValidationStatus::Valid,
            config_version: config_version.or(Some(2)), // Default to 2 if not specified but valid
            detected_version: package_version,
            is_valid_v2: true,
            can_be_converted: false,
            message: "Valid V2 soundpack configuration".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bundled packs store config_version as a string.
    const REAL_PACK_CONFIG: &str = r#"{
            "audio_file": "sound.ogg",
            "config_version": "2",
            "created_at": "2025-06-17T12:23:39.537516300+00:00",
            "definition_method": "single",
            "author": "sorakey",
            "definitions": {
                "AltLeft": { "timing": [[45750.0, 45832.0], [45832.0, 45914.0]] },
                "Escape": { "timing": [[2894.0, 3007.0], [3007.0, 3120.0]] }
            },
            "id": "keyboard/sample-pack",
            "name": "Sample Pack",
            "options": { "random_pitch": false, "recommended_volume": 1.0 },
            "icon": "black.jpg",
            "tags": []
        }"#;

    fn validate_json(contents: &str) -> SoundpackValidationResult {
        let path = std::env::temp_dir().join(format!(
            "sorakey-validator-{}-{:?}-{}.json",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, contents).expect("write temp config");
        let result = validate_soundpack_config(path.to_str().expect("utf-8 temp path"));
        std::fs::remove_file(&path).ok();
        result
    }

    fn version_of(raw: &str) -> Option<u32> {
        read_config_version(&serde_json::from_str(raw).expect("valid json"))
    }

    #[test]
    fn config_version_is_read_from_both_number_and_string_forms() {
        assert_eq!(version_of(r#"{"config_version": 2}"#), Some(2));
        assert_eq!(version_of(r#"{"config_version": "2"}"#), Some(2));
        assert_eq!(version_of(r#"{"config_version": 1}"#), Some(1));
        assert_eq!(version_of(r#"{"config_version": "1"}"#), Some(1));
        assert_eq!(version_of(r#"{"config_version": 3}"#), Some(3));
        assert_eq!(version_of(r#"{"config_version": "3"}"#), Some(3));
    }

    #[test]
    fn an_unreadable_config_version_is_treated_as_absent() {
        assert_eq!(version_of(r#"{}"#), None, "absent");
        assert_eq!(version_of(r#"{"config_version": null}"#), None, "null");
        assert_eq!(
            version_of(r#"{"config_version": "banana"}"#),
            None,
            "garbage text"
        );
        assert_eq!(
            version_of(r#"{"config_version": ""}"#),
            None,
            "empty string"
        );
        assert_eq!(version_of(r#"{"config_version": -1}"#), None, "negative");
        assert_eq!(version_of(r#"{"config_version": 2.5}"#), None, "fractional");
    }

    /// Real pack must validate as V2.
    #[test]
    fn a_real_bundled_pack_validates_as_v2() {
        let result = validate_json(REAL_PACK_CONFIG);

        assert_eq!(
            result.status,
            SoundpackValidationStatus::Valid,
            "a shipped pack must validate; got: {}",
            result.message
        );
        assert_eq!(
            result.config_version,
            Some(2),
            "the string \"2\" must be understood as version 2"
        );
        assert!(result.is_valid_v2);
        assert!(
            !result.can_be_converted,
            "a valid V2 pack needs no conversion"
        );
    }

    /// Every bundled pack must validate.
    #[test]
    fn every_bundled_soundpack_validates() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("soundpacks");

        let mut checked = 0;
        let mut dirs = vec![root.clone()];
        while let Some(dir) = dirs.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => {
                    continue;
                }
            };

            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                    let result = validate_soundpack_config(path.to_str().expect("utf-8 path"));
                    assert_eq!(
                        result.status,
                        SoundpackValidationStatus::Valid,
                        "bundled pack {} must validate; got: {}",
                        path.display(),
                        result.message
                    );
                    assert!(
                        result.is_valid_v2,
                        "bundled pack {} must be valid V2",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }

        assert!(
            checked > 0,
            "expected to find bundled soundpack configs under {}",
            root.display()
        );
    }

    /// `defs` alias is accepted.
    #[test]
    fn the_defs_spelling_is_accepted_as_an_alias() {
        let result = validate_json(
            r#"{
                "config_version": 2,
                "name": "alias pack",
                "author": "someone",
                "defs": { "Escape": [[0.0, 100.0]] }
            }"#,
        );

        assert_eq!(
            result.status,
            SoundpackValidationStatus::Valid,
            "got: {}",
            result.message
        );
    }

    /// Future version asks to update.
    #[test]
    fn a_newer_config_version_asks_the_user_to_update() {
        for raw in ["3", "\"3\"", "99"] {
            let result = validate_json(&format!(
                r#"{{"config_version": {}, "name": "future", "author": "a"}}"#,
                raw
            ));

            assert!(
                matches!(
                    result.status,
                    SoundpackValidationStatus::RequiresNewerAppVersion(_)
                ),
                "config_version {} must report a newer app version, got {:?}",
                raw,
                result.status
            );
            assert_eq!(
                result.message,
                "This soundpack requires a newer version of Sorakey"
            );
            assert!(
                !result.can_be_converted,
                "a future format cannot be converted by this build"
            );
            assert!(!result.is_valid_v2);
        }
    }

    /// Future pack isn't mistaken for V1.
    #[test]
    fn a_newer_version_is_not_mistaken_for_a_convertible_v1_pack() {
        let result = validate_json(
            r#"{
                "config_version": "3",
                "name": "future",
                "defines": { "1": [0, 100] },
                "sound": "sound.ogg"
            }"#,
        );

        assert!(
            matches!(
                result.status,
                SoundpackValidationStatus::RequiresNewerAppVersion(_)
            ),
            "got {:?}",
            result.status
        );
        assert!(
            !result.can_be_converted,
            "converting a format we cannot read would corrupt it"
        );
    }

    /// V1 packs need conversion.
    #[test]
    fn a_v1_pack_still_asks_to_be_converted() {
        let result = validate_json(
            r#"{
                "config_version": 1,
                "name": "old pack",
                "defines": { "1": [0, 100] },
                "sound": "sound.ogg"
            }"#,
        );

        assert_eq!(
            result.status,
            SoundpackValidationStatus::VersionOneNeedsConversion
        );
        assert!(result.can_be_converted);
    }

    /// Unversioned V1 detected by shape.
    #[test]
    fn an_unversioned_v1_pack_is_detected_by_structure() {
        let result = validate_json(
            r#"{ "name": "old", "defines": { "1": [0, 100] }, "sound": "sound.ogg" }"#,
        );

        assert_eq!(
            result.status,
            SoundpackValidationStatus::VersionOneNeedsConversion
        );
        assert!(result.can_be_converted);
    }

    /// Missing definitions are reported.
    #[test]
    fn a_config_with_no_definitions_reports_the_missing_field() {
        let result = validate_json(r#"{ "name": "empty" }"#);

        match result.status {
            SoundpackValidationStatus::MissingRequiredFields(fields) => {
                assert!(
                    fields.iter().any(|f| f.contains("definitions")),
                    "the missing field should be named as packs spell it, got {:?}",
                    fields
                );
            }
            other => panic!("expected missing required fields, got {:?}", other),
        }
    }
}
