use super::files;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Get audio duration in ms.
fn get_audio_duration_ms(file_path: &str) -> Result<f64, Box<dyn std::error::Error>> {
    if !Path::new(file_path).exists() {
        return Err("File does not exist".into());
    } // Use symphonia for audio duration detection
    match get_duration_with_symphonia(file_path) {
        Ok(duration) if duration > 0.0 => Ok(duration),
        Ok(_) => Ok(100.0),
        Err(_) => Ok(100.0),
    }
}

/// Get duration via shared Symphonia helper.
fn get_duration_with_symphonia(file_path: &str) -> Result<f64, Box<dyn std::error::Error>> {
    Ok(crate::utils::sound_reader::duration_ms(file_path).unwrap_or(100.0))
}

/// JSON number for a computed `f64`. `Number::from_f64` returns `None` for
/// NaN/Inf (corrupt V1 timings can produce those); unwrapping would panic
/// the converter, so fall back to 0.0 instead.
fn json_num(value: f64) -> Value {
    Value::Number(
        serde_json::Number::from_f64(value)
            .unwrap_or_else(|| serde_json::Number::from_f64(0.0).expect("0.0 is finite")),
    )
}

/// Convert V1 config to V2.
pub fn convert_v1_to_v2(
    v1_config_path: &str,
    output_path: &str,
    soundpack_dir: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let soundpack_dir = soundpack_dir.unwrap_or(
        Path::new(v1_config_path)
            .parent()
            .and_then(|p| p.to_str())
            .ok_or("Could not determine soundpack directory")?,
    );

    let content = files::read_file_contents(v1_config_path)
        .map_err(|e| format!("Failed to read V1 config: {}", e))?;
    let config: Value = serde_json::from_str(&content)?;

    let mut converted_config = Map::new();

    if let Some(id) = config.get("id") {
        converted_config.insert("id".to_string(), id.clone());
    }

    if let Some(name) = config.get("name") {
        converted_config.insert("name".to_string(), name.clone());
    }

    if let Some(description) = config.get("description") {
        converted_config.insert("description".to_string(), description.clone());
    }

    if let Some(author) = config.get("author") {
        converted_config.insert("author".to_string(), author.clone());
    }

    if let Some(version) = config.get("version") {
        converted_config.insert("version".to_string(), version.clone());
    }

    converted_config.insert("config_version".to_string(), Value::String("2".to_string()));

    if let Some(icon) = config.get("icon") {
        converted_config.insert("icon".to_string(), icon.clone());
    }

    if let Some(tags) = config.get("tags") {
        converted_config.insert("tags".to_string(), tags.clone());
    }

    let now = chrono::Utc::now();
    converted_config.insert("created_at".to_string(), Value::String(now.to_rfc3339())); // Determine definition_method from V1 key_define_type or sound structure
    let v1_define_type = config
        .get("key_define_type")
        .and_then(|v| v.as_str())
        .unwrap_or("single");
    let definition_method = "single";

    converted_config.insert(
        "definition_method".to_string(),
        Value::String(definition_method.to_string()),
    ); // Handle audio_file for "single" method
    let (audio_file_name, audio_file_info) = if v1_define_type == "multi" {
        let mut audio_files_ordered = Vec::new();
        let mut seen_files = std::collections::HashSet::new();

        if let Some(defines) = config.get("defines").and_then(|d| d.as_object()) {
            // Press before release, then numeric order. A bare code sort ties
            // press "30" against its release "030", and HashMap iteration order
            // then decided which file survived the dedupe below — random
            // concatenation order per run.
            let mut sorted_keys: Vec<_> = defines.keys().collect();
            sorted_keys.sort_by_key(|k| {
                let (num, is_press) = iohook_code_and_press(k).unwrap_or((u32::MAX, false));
                (!is_press, num)
            });

            for key in sorted_keys {
                if let Some(value) = defines.get(key) {
                    if let Some(filename) = value.as_str() {
                        if !filename.is_empty()
                            && filename != "null"
                            && !seen_files.contains(filename)
                        {
                            audio_files_ordered.push(filename.to_string());
                            seen_files.insert(filename.to_string());
                        }
                    }
                }
            }
        }

        crate::always_print!(
            "🔧 Found {} unique audio files in V1 multi method",
            audio_files_ordered.len()
        );

        let concat_filename = "concatenated_audio.wav";
        crate::always_print!("🎵 Creating concatenated audio file: {}", concat_filename);

        let audio_file_info = match concatenate_audio_files_with_timing(
            &audio_files_ordered,
            soundpack_dir,
            concat_filename,
        ) {
            Ok(timing_info) => timing_info,
            Err(e) => {
                crate::always_print!("❌ Failed to create concatenated audio file: {}", e);
                return Err(format!("Audio concatenation failed: {}", e).into());
            }
        };

        (concat_filename.to_string(), audio_file_info)
    } else {
        let main_file = if let Some(sound) = config.get("sound") {
            if let Some(sound_str) = sound.as_str() {
                crate::always_print!(
                    "🎵 Using main audio file from V1 single method: {}",
                    sound_str
                );
                sound_str.to_string()
            } else {
                return Err("Invalid sound field in V1 config".into());
            }
        } else {
            let audio_extensions = ["ogg", "mp3", "wav", "flac"];
            let mut found_audio = None;

            if let Ok(entries) = std::fs::read_dir(soundpack_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    if let Some(filename) = entry.file_name().to_str() {
                        let filename_lower = filename.to_lowercase();
                        for ext in &audio_extensions {
                            if filename_lower.ends_with(&format!(".{}", ext)) {
                                found_audio = Some(filename.to_string());
                                break;
                            }
                        }
                        if found_audio.is_some() {
                            break;
                        }
                    }
                }
            }

            if let Some(audio_file) = found_audio {
                crate::always_print!("🎵 Found audio file in directory: {}", audio_file);
                audio_file
            } else {
                return Err("No audio file found for single method conversion".into());
            }
        };

        (main_file, std::collections::HashMap::new())
    };

    converted_config.insert(
        "audio_file".to_string(),
        Value::String(audio_file_name.clone()),
    );

    let mut options = Map::new();
    options.insert("recommended_volume".to_string(), json_num(1.0));
    options.insert("random_pitch".to_string(), Value::Bool(false));
    converted_config.insert("options".to_string(), Value::Object(options)); // Convert "defines" to "definitions" with new format
    let mut definitions = Map::new();
    if let Some(defines) = config.get("defines").and_then(|d| d.as_object()) {
        let key_mappings = create_iohook_to_web_key_mapping();
        crate::always_print!(
            "🔧 Converting {} key definitions to new format (single method)",
            defines.len()
        );
        if v1_define_type == "multi" {
            crate::always_print!("🔧 Processing V1 multi method defines");

            let mut ordered: Vec<(&String, &Value)> = defines.iter().collect();
            ordered.sort_by_key(|(code, _)| {
                let (num, is_press) = iohook_code_and_press(code).unwrap_or((u32::MAX, true));
                (!is_press, num)
            });

            for (iohook_code, value) in ordered {
                if let Some((iohook_num, _)) = iohook_code_and_press(iohook_code) {
                    if let Some(key_name) = key_mappings.get(&iohook_num) {
                        if let Some(audio_filename) = value.as_str() {
                            if !audio_filename.is_empty() && audio_filename != "null" {
                                let mut key_def = Map::new();

                                if let Some(&(offset, duration)) =
                                    audio_file_info.get(audio_filename)
                                {
                                    let end_time = offset + duration;

                                    if key_name == "Enter" {
                                        crate::always_print!("🔍 [ENTER DEBUG] Key: {}", key_name);
                                        crate::always_print!(
                                            "🔍 [ENTER DEBUG] IOHook code: {}",
                                            iohook_num
                                        );
                                        crate::always_print!(
                                            "🔍 [ENTER DEBUG] Audio file: {}",
                                            audio_filename
                                        );
                                        crate::always_print!(
                                            "🔍 [ENTER DEBUG] Offset: {}ms",
                                            offset
                                        );
                                        crate::always_print!(
                                            "🔍 [ENTER DEBUG] Duration: {}ms",
                                            duration
                                        );
                                        crate::always_print!(
                                            "🔍 [ENTER DEBUG] End time: {}ms",
                                            end_time
                                        );

                                        let concat_path =
                                            format!("{}/concatenated_audio.wav", soundpack_dir);
                                        if let Ok(concat_duration) =
                                            get_audio_duration_ms(&concat_path)
                                        {
                                            crate::always_print!(
                                                "🔍 [ENTER DEBUG] Concatenated audio duration: {}ms",
                                                concat_duration
                                            );
                                            if end_time > concat_duration {
                                                crate::always_print!(
                                                    "❌ [ENTER DEBUG] ERROR: End time ({}) > Concat duration ({})",
                                                    end_time,
                                                    concat_duration
                                                );
                                            }
                                        }
                                    }

                                    let timing = vec![Value::Array(vec![
                                        json_num(offset),
                                        json_num(end_time),
                                    ])];
                                    key_def
                                        .insert("timing".to_string(), Value::Array(timing.clone()));

                                    match definitions.get_mut(key_name.as_str()) {
                                        Some(Value::Object(existing)) => {
                                            if let Some(Value::Array(segments)) =
                                                existing.get_mut("timing")
                                            {
                                                segments.extend(timing);
                                            }
                                        }
                                        _ => {
                                            definitions
                                                .insert(key_name.clone(), Value::Object(key_def));
                                        }
                                    }
                                    crate::always_print!(
                                        "   ✅ Key '{}' -> {} [offset: {}ms, end: {}ms]",
                                        key_name,
                                        audio_filename,
                                        offset,
                                        end_time
                                    );
                                } else {
                                    crate::always_print!(
                                        "   ⚠️ No offset found for audio file: {}",
                                        audio_filename
                                    );
                                }
                            } else {
                                crate::always_print!(
                                    "   ⚠️ Key IOHook {} has empty/null audio file",
                                    iohook_code
                                );
                            }
                        }
                    } else {
                        crate::always_print!(
                            "   ⚠️ No key mapping found for IOHook code: {}",
                            iohook_code
                        );
                    }
                }
            }
        } else {
            crate::always_print!("🔧 Processing V1 single method defines");

            // Press before release, then numeric order (same as the multi
            // path): defines "30" (press) and "030" (release) share one key
            // name, and the engine plays timing[0] as press / timing[1] as
            // release. A plain `definitions.insert` here used to let the
            // second one silently overwrite the first (HashMap order decided
            // which segment survived), so existing entries are extended.
            let mut ordered: Vec<(&String, &Value)> = defines.iter().collect();
            ordered.sort_by_key(|(code, _)| {
                let (num, is_press) = iohook_code_and_press(code).unwrap_or((u32::MAX, true));
                (!is_press, num)
            });

            for (iohook_code, value) in ordered {
                if let Some((iohook_num, _)) = iohook_code_and_press(iohook_code) {
                    if let Some(key_name) = key_mappings.get(&iohook_num) {
                        if let Some(timing_array) = value.as_array() {
                            if timing_array.len() >= 2 {
                                let start = timing_array[0].as_f64().unwrap_or(0.0) as f32;
                                let duration = timing_array[1].as_f64().unwrap_or(100.0) as f32;
                                let end = start + duration;

                                let timing = vec![Value::Array(vec![
                                    json_num(start as f64),
                                    json_num(end as f64),
                                ])];
                                match definitions.get_mut(key_name.as_str()) {
                                    Some(Value::Object(existing)) => {
                                        if let Some(Value::Array(segments)) =
                                            existing.get_mut("timing")
                                        {
                                            segments.extend(timing);
                                        }
                                    }
                                    _ => {
                                        let mut key_def = Map::new();
                                        key_def.insert("timing".to_string(), Value::Array(timing));
                                        definitions
                                            .insert(key_name.clone(), Value::Object(key_def));
                                    }
                                }
                                crate::always_print!(
                                    "   ✅ Key '{}' -> timing [{}, {}]",
                                    key_name,
                                    start,
                                    end
                                );
                            }
                        } else {
                            crate::always_print!(
                                "   ⚠️ Key '{}' has invalid timing format",
                                key_name
                            );
                        }
                    }
                }
            }
        }
    }

    converted_config.insert("definitions".to_string(), Value::Object(definitions));

    let output_json = serde_json::to_string_pretty(&converted_config)?;
    write_atomic_string(output_path, &output_json)?;

    crate::always_print!("✅ Successfully converted V1 to V2 config");
    crate::always_print!("📁 Output written to: {}", output_path);

    Ok(())
}

/// Write `contents` to `path` atomically (temp file in the same directory +
/// rename) so a crash mid-conversion never leaves a truncated config behind.
/// Same-directory temp keeps the rename on one filesystem (truly atomic).
fn write_atomic_string(path: &str, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = format!("{}.tmp.{}", path, std::process::id());
    std::fs::write(&tmp, contents)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Move existing file aside before overwriting — returns backup path.
pub fn back_up_existing_file(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let base = {
        let mut name = path.as_os_str().to_os_string();
        name.push(".bak");
        PathBuf::from(name)
    };

    let mut target = base.clone();
    let mut attempt = 1;
    while target.exists() {
        let mut name = base.as_os_str().to_os_string();
        name.push(format!(".{}", attempt));
        target = PathBuf::from(name);
        attempt += 1;

        if attempt > 100 {
            return Err(format!(
                "too many backups already exist next to {}",
                path.display()
            ));
        }
    }

    std::fs::copy(path, &target).map_err(|e| {
        format!(
            "failed to back up {} to {}: {}",
            path.display(),
            target.display(),
            e
        )
    })?;

    crate::always_print!(
        "🗄️  Backed up existing {} to {}",
        path.display(),
        target.display()
    );
    Ok(Some(target))
}

/// Concatenate audio files, return timing per file.
fn concatenate_audio_files_with_timing(
    audio_files: &[String], // just filenames
    soundpack_dir: &str,
    output_filename: &str,
) -> Result<std::collections::HashMap<String, (f64, f64)>, Box<dyn std::error::Error>> {
    crate::always_print!(
        "🔧 Concatenating {} audio files with timing...",
        audio_files.len()
    );

    let mut all_samples = Vec::new();
    let mut sample_rate = 44100u32; // Default sample rate
    let mut channels = 2u16; // Default to stereo
    let mut timing_info = std::collections::HashMap::new();

    for (i, filename) in audio_files.iter().enumerate() {
        let file_path = format!("{}/{}", soundpack_dir, filename);
        crate::always_print!(
            "   📁 Loading audio file {}/{}: {}",
            i + 1,
            audio_files.len(),
            filename
        );

        if !Path::new(&file_path).exists() {
            crate::always_print!("   ⚠️ Audio file not found, skipping: {}", file_path);
            continue;
        }

        let current_offset_ms =
            ((all_samples.len() as f64) / ((sample_rate as f64) * (channels as f64))) * 1000.0;

        match load_audio_file_samples(&file_path) {
            Ok((samples, file_channels, file_sample_rate)) => {
                if i == 0 {
                    sample_rate = file_sample_rate;
                    channels = file_channels;
                    crate::always_print!(
                        "   🎵 Using format: {}Hz, {} channels",
                        sample_rate,
                        channels
                    );
                }

                let converted_samples =
                    if file_sample_rate != sample_rate || file_channels != channels {
                        crate::always_print!(
                            "   🔄 Converting from {}Hz {} channels to {}Hz {} channels",
                            file_sample_rate,
                            file_channels,
                            sample_rate,
                            channels
                        );
                        convert_audio_format(
                            &samples,
                            file_channels,
                            file_sample_rate,
                            channels,
                            sample_rate,
                        )
                    } else {
                        samples
                    };

                let actual_duration_ms = ((converted_samples.len() as f64)
                    / ((sample_rate as f64) * (channels as f64)))
                    * 1000.0;

                timing_info.insert(filename.clone(), (current_offset_ms, actual_duration_ms));

                if filename == "SPMEnter.wav" {
                    crate::always_print!("🔍 [ENTER TIMING DEBUG] File: {}", filename);
                    crate::always_print!(
                        "🔍 [ENTER TIMING DEBUG] Offset: {:.2}ms",
                        current_offset_ms
                    );
                    crate::always_print!(
                        "🔍 [ENTER TIMING DEBUG] Duration: {:.2}ms",
                        actual_duration_ms
                    );
                    crate::always_print!(
                        "🔍 [ENTER TIMING DEBUG] End time: {:.2}ms",
                        current_offset_ms + actual_duration_ms
                    );
                    crate::always_print!(
                        "🔍 [ENTER TIMING DEBUG] Samples: {}",
                        converted_samples.len()
                    );
                }

                all_samples.extend(&converted_samples);
                crate::always_print!(
                    "   ✅ Added {} samples from {} (offset: {:.2}ms, duration: {:.2}ms)",
                    converted_samples.len(),
                    filename,
                    current_offset_ms,
                    actual_duration_ms
                );
            }
            Err(e) => {
                crate::always_print!("   ❌ Failed to load {}: {}", filename, e);
            }
        }
    }

    if all_samples.is_empty() {
        return Err("No audio samples were loaded".into());
    }

    let output_path = format!("{}/{}", soundpack_dir, output_filename);
    back_up_existing_file(Path::new(&output_path))?;
    save_audio_file(&all_samples, channels, sample_rate, &output_path)?;

    let final_duration_ms =
        ((all_samples.len() as f64) / ((sample_rate as f64) * (channels as f64))) * 1000.0;

    crate::always_print!("✅ Successfully concatenated audio to: {}", output_path);
    crate::always_print!(
        "🎵 Total samples: {}, Final duration: {:.2}ms",
        all_samples.len(),
        final_duration_ms
    );

    if let Some((offset, duration)) = timing_info.get("SPMEnter.wav") {
        crate::always_print!(
            "🔍 [FINAL TIMING DEBUG] SPMEnter.wav: offset={:.2}ms, duration={:.2}ms, end={:.2}ms",
            offset,
            duration,
            offset + duration
        );
        crate::always_print!(
            "🔍 [FINAL TIMING DEBUG] Concatenated total: {:.2}ms",
            final_duration_ms
        );
        if offset + duration > final_duration_ms {
            crate::always_print!("❌ [FINAL TIMING DEBUG] ERROR: End time exceeds total duration!");
        } else {
            crate::always_print!("✅ [FINAL TIMING DEBUG] Timing looks correct!");
        }
    }

    Ok(timing_info)
}

/// Load audio file samples via shared decoder.
fn load_audio_file_samples(
    file_path: &str,
) -> Result<(Vec<f32>, u16, u32), Box<dyn std::error::Error>> {
    crate::utils::sound_reader::decode_interleaved(file_path).map_err(|e| e.into())
}

/// Convert channels and rate — both matter for correct speed.
fn convert_audio_format(
    samples: &[f32],
    from_channels: u16,
    from_sample_rate: u32,
    to_channels: u16,
    to_sample_rate: u32,
) -> Vec<f32> {
    if from_channels == to_channels && from_sample_rate == to_sample_rate {
        return samples.to_vec();
    }

    let channel_converted = convert_channels(samples, from_channels, to_channels);

    if from_sample_rate == to_sample_rate {
        return channel_converted;
    }

    crate::libs::sound_quality::resample_interleaved(
        &channel_converted,
        to_channels.max(1),
        from_sample_rate,
        to_sample_rate,
    )
}

/// Remap channel layout, keeping frame count.
fn convert_channels(samples: &[f32], from_channels: u16, to_channels: u16) -> Vec<f32> {
    let from = from_channels.max(1) as usize;
    let to = to_channels.max(1) as usize;

    if from == to {
        return samples.to_vec();
    }

    if from == 1 && to == 2 {
        return samples
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect();
    }

    if from == 2 && to == 1 {
        return samples
            .chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    (chunk[0] + chunk[1]) / 2.0
                } else {
                    chunk[0]
                }
            })
            .collect();
    }

    let mut out = Vec::with_capacity((samples.len() / from) * to);
    for frame in samples.chunks(from) {
        let mono = frame.iter().sum::<f32>() / (frame.len() as f32);
        for _ in 0..to {
            out.push(mono);
        }
    }
    out
}

/// Save samples as WAV, atomically: encode to a sibling temp file and rename
/// over the target, so a crash can never leave a truncated WAV behind (the
/// old `WavWriter::create` straight at the target truncated it first).
fn save_audio_file(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    output_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use hound;

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let tmp_path = format!("{}.tmp.{}", output_path, std::process::id());
    {
        let mut writer = hound::WavWriter::create(&tmp_path, spec)?;

        for &sample in samples {
            let sample_i16 = (sample * (i16::MAX as f32)) as i16;
            writer.write_sample(sample_i16)?;
        }

        writer.finalize()?;
    }
    if let Err(e) = std::fs::rename(&tmp_path, output_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    Ok(())
}

/// Split V1 key: leading zero means release.
fn iohook_code_and_press(define_key: &str) -> Option<(u32, bool)> {
    let is_press = !(define_key.len() > 1 && define_key.starts_with('0'));
    let code = define_key.parse::<u32>().ok()?;
    Some((code, is_press))
}

fn create_iohook_to_web_key_mapping() -> HashMap<u32, String> {
    // Standard range (codes 1-88) shares its codes with the Linux input-event
    // table in utils/keymap.rs; seeding from it keeps the runtime listener and
    // the converter on one source of truth. Everything below is IOHook-only
    // codes that evdev does not use at the same value.
    let mut mapping: HashMap<u32, String> = crate::utils::keys::KEY_MAP
        .iter()
        .map(|&(code, name)| (code as u32, name.to_string()))
        .collect();

    mapping.insert(91, "F13".to_string()); // VC_F13 = 0x005B
    mapping.insert(92, "F14".to_string()); // VC_F14 = 0x005C
    mapping.insert(93, "F15".to_string()); // VC_F15 = 0x005D
    mapping.insert(99, "F16".to_string()); // VC_F16 = 0x0063
    mapping.insert(100, "F17".to_string()); // VC_F17 = 0x0064
    mapping.insert(101, "F18".to_string()); // VC_F18 = 0x0065
    mapping.insert(102, "F19".to_string()); // VC_F19 = 0x0066
    mapping.insert(103, "F20".to_string()); // VC_F20 = 0x0067
    mapping.insert(104, "F21".to_string()); // VC_F21 = 0x0068
    mapping.insert(105, "F22".to_string()); // VC_F22 = 0x0069
    mapping.insert(106, "F23".to_string()); // VC_F23 = 0x006A
    mapping.insert(107, "F24".to_string()); // VC_F24 = 0x006B

    mapping.insert(112, "Convert".to_string()); // VC_KATAKANA = 0x0070
    mapping.insert(115, "Lang1".to_string()); // VC_UNDERSCORE = 0x0073
    mapping.insert(119, "Lang2".to_string()); // VC_FURIGANA = 0x0077
    mapping.insert(121, "KanaMode".to_string()); // VC_KANJI = 0x0079
    mapping.insert(123, "HiraganaKatakana".to_string()); // VC_HIRAGANA = 0x007B
    mapping.insert(125, "IntlYen".to_string()); // VC_YEN = 0x007D
    mapping.insert(126, "NumpadComma".to_string()); // VC_KP_COMMA = 0x007E

    mapping.insert(3637, "NumpadDivide".to_string()); // VC_KP_DIVIDE = 0x0E35
    mapping.insert(3612, "NumpadEnter".to_string()); // VC_KP_ENTER = 0x0E1C
    mapping.insert(3597, "ControlRight".to_string()); // VC_CONTROL_R = 0x0E1D
    mapping.insert(3645, "NumpadEquals".to_string()); // VC_KP_EQUALS = 0x0E0D    // Navigation cluster - using CORRECT 0xE0xx values (fixed from incorrect mapping)
    mapping.insert(57399, "PrintScreen".to_string()); // VC_PRINTSCREEN = 0xE037 = 57399
    mapping.insert(58437, "Pause".to_string()); // VC_PAUSE = 0xE045 = 57413 (keeping old for compatibility)
    mapping.insert(57415, "Home".to_string()); // VC_HOME = 0xE047 = 57415
    mapping.insert(57416, "ArrowUp".to_string()); // VC_UP = 0xE048 = 57416 ✓ CORRECT
    mapping.insert(57417, "PageUp".to_string()); // VC_PAGE_UP = 0xE049 = 57417
    mapping.insert(57419, "ArrowLeft".to_string()); // VC_LEFT = 0xE04B = 57419 ✓ CORRECT
    mapping.insert(57421, "ArrowRight".to_string()); // VC_RIGHT = 0xE04D = 57421 ✓ CORRECT
    mapping.insert(57423, "End".to_string()); // VC_END = 0xE04F = 57423
    mapping.insert(57424, "ArrowDown".to_string()); // VC_DOWN = 0xE050 = 57424 ✓ CORRECT
    mapping.insert(57425, "PageDown".to_string()); // VC_PAGE_DOWN = 0xE051 = 57425
    mapping.insert(57426, "Insert".to_string()); // VC_INSERT = 0xE052 = 57426
    mapping.insert(57427, "Delete".to_string()); // VC_DELETE = 0xE053 = 57427    // Extended modifier keys - using CORRECT IOHook values
    mapping.insert(57400, "AltRight".to_string()); // VC_ALT_R = 0xE038 = 57400
    mapping.insert(57435, "MetaLeft".to_string()); // VC_META_L = 0xE05B = 57435
    mapping.insert(57436, "MetaRight".to_string()); // VC_META_R = 0xE05C = 57436
    mapping.insert(57437, "ContextMenu".to_string()); // VC_CONTEXT_MENU = 0xE05D = 57437    // Power and sleep keys - using CORRECT IOHook values
    mapping.insert(57438, "Power".to_string()); // VC_POWER = 0xE05E = 57438
    mapping.insert(57439, "Sleep".to_string()); // VC_SLEEP = 0xE05F = 57439
    mapping.insert(57443, "WakeUp".to_string()); // VC_WAKE = 0xE063 = 57443

    mapping.insert(57360, "MediaTrackPrevious".to_string()); // VC_MEDIA_PREVIOUS = 0xE010
    mapping.insert(57369, "MediaTrackNext".to_string()); // VC_MEDIA_NEXT = 0xE019
    mapping.insert(57376, "AudioVolumeMute".to_string()); // VC_VOLUME_MUTE = 0xE020
    mapping.insert(57377, "LaunchApp2".to_string()); // VC_APP_CALCULATOR = 0xE021
    mapping.insert(57378, "MediaPlayPause".to_string()); // VC_MEDIA_PLAY = 0xE022
    mapping.insert(57380, "MediaStop".to_string()); // VC_MEDIA_STOP = 0xE024
    mapping.insert(57390, "AudioVolumeDown".to_string()); // VC_VOLUME_DOWN = 0xE02E
    mapping.insert(57392, "AudioVolumeUp".to_string()); // VC_VOLUME_UP = 0xE030
    mapping.insert(57394, "BrowserHome".to_string()); // VC_BROWSER_HOME = 0xE032
    mapping.insert(57404, "LaunchApp1".to_string()); // VC_APP_MUSIC = 0xE03C
    mapping.insert(57444, "LaunchApp3".to_string()); // VC_APP_PICTURES = 0xE064
    mapping.insert(57445, "BrowserSearch".to_string()); // VC_BROWSER_SEARCH = 0xE065
    mapping.insert(57446, "BrowserFavorites".to_string()); // VC_BROWSER_FAVORITES = 0xE066
    mapping.insert(57447, "BrowserRefresh".to_string()); // VC_BROWSER_REFRESH = 0xE067
    mapping.insert(57448, "BrowserStop".to_string()); // VC_BROWSER_STOP = 0xE068
    mapping.insert(57449, "BrowserForward".to_string()); // VC_BROWSER_FORWARD = 0xE069
    mapping.insert(57450, "BrowserBack".to_string()); // VC_BROWSER_BACK = 0xE06A
    mapping.insert(57452, "LaunchMail".to_string()); // VC_APP_MAIL = 0xE06C
    mapping.insert(57453, "MediaSelect".to_string()); // VC_MEDIA_SELECT = 0xE06D

    mapping.insert(58444, "Clear".to_string()); // VC_CLEAR = 0xE04C (alternate)
    mapping.insert(58470, "IntlBackslash".to_string()); // VC_LESSER_GREATER = 0xE046

    // Alternative 0x0Exx extended-range codes some V1 packs use instead of the
    // main block's (3637/3612/3597/3645 live in the main block). Values are
    // copied from scripts/_v1_shared.py — the executed source of truth — so
    // the two conversion paths agree. The old "Alternative range" block
    // corrupted 3597/3612/3640 by overwriting them with numpad names; the
    // parity test now pins both tables to stay identical.
    mapping.insert(3613, "ControlRight".to_string()); // ControlRight alt
    mapping.insert(3639, "Numpad7".to_string()); // Numpad7 alt
    mapping.insert(3640, "AltRight".to_string()); // AltRight alt
    mapping.insert(3653, "Numpad9".to_string()); // Numpad9 alt
    mapping.insert(3655, "NumpadAdd".to_string()); // NumpadAdd alt
    mapping.insert(3657, "Numpad4".to_string()); // Numpad4 alt
    mapping.insert(3663, "Numpad5".to_string()); // Numpad5 alt
    mapping.insert(3665, "Numpad6".to_string()); // Numpad6 alt
    mapping.insert(3666, "Numpad1".to_string()); // Numpad1 alt
    mapping.insert(3667, "Numpad2".to_string()); // Numpad2 alt
    mapping.insert(3675, "NumpadDecimal".to_string()); // NumpadDecimal alt
    mapping.insert(3676, "Numpad0".to_string()); // Numpad0 alt

    mapping.insert(60999, "Insert".to_string()); // V1 compatibility
    mapping.insert(61000, "Delete".to_string()); // V1 compatibility
    mapping.insert(61001, "Home".to_string()); // V1 compatibility
    mapping.insert(61003, "End".to_string()); // V1 compatibility
    mapping.insert(61005, "PageUp".to_string()); // V1 compatibility
    mapping.insert(61007, "PageDown".to_string()); // V1 compatibility
    mapping.insert(61008, "PrintScreen".to_string()); // V1 compatibility
    mapping.insert(61009, "ScrollLock".to_string()); // V1 compatibility
    mapping.insert(61010, "Pause".to_string()); // V1 compatibility
    mapping.insert(61011, "NumpadDecimal".to_string()); // V1 compatibility

    mapping.insert(94, "IntlBackslash".to_string()); // Less/Greater key on some keyboards
    mapping.insert(95, "Fn".to_string()); // Function key modifier
    mapping.insert(96, "Clear".to_string()); // Clear key on some keyboards

    mapping.insert(65397, "Help".to_string()); // VC_SUN_HELP = 0xFF75
    mapping.insert(65398, "Props".to_string()); // VC_SUN_PROPS = 0xFF76
    mapping.insert(65399, "Front".to_string()); // VC_SUN_FRONT = 0xFF77
    mapping.insert(65400, "Stop".to_string()); // VC_SUN_STOP = 0xFF78
    mapping.insert(65401, "Again".to_string()); // VC_SUN_AGAIN = 0xFF79
    mapping.insert(65402, "Undo".to_string()); // VC_SUN_UNDO = 0xFF7A
    mapping.insert(65403, "Cut".to_string()); // VC_SUN_CUT = 0xFF7B
    mapping.insert(65404, "Copy".to_string()); // VC_SUN_COPY = 0xFF7C
    mapping.insert(65405, "Paste".to_string()); // VC_SUN_INSERT = 0xFF7D
    mapping.insert(65406, "Find".to_string()); // VC_SUN_FIND = 0xFF7E

    mapping
}

#[cfg(test)]
mod tests {
    use super::{
        back_up_existing_file, concatenate_audio_files_with_timing, convert_audio_format,
        create_iohook_to_web_key_mapping,
    };

    /// The 0x0Exx "alternative" codes must resolve to the same names the
    /// Python importer produces (scripts/_v1_shared.py, executed
    /// last-wins). This is what kept the two conversion paths in lockstep
    /// before the "Alternative range" block silently overwrote 3597/3612/
    /// 3613/3640 with wrong numpad names.
    #[test]
    fn v1_table_matches_the_python_importer_on_the_conflicting_codes() {
        let m = create_iohook_to_web_key_mapping();
        assert_eq!(m[&3597], "ControlRight");
        assert_eq!(m[&3612], "NumpadEnter");
        assert_eq!(m[&3613], "ControlRight");
        assert_eq!(m[&3640], "AltRight");
        assert_eq!(m[&3675], "NumpadDecimal");
        assert_eq!(m[&3676], "Numpad0");
        assert!(!m.contains_key(&3677), "3677 has no Python counterpart");
    }

    /// Full lockstep: every Rust entry must equal the Python final table.
    /// Run with python3 available (dev machines and CI both have it).
    #[test]
    fn rust_v1_table_stays_in_lockstep_with_python() {
        let output = std::process::Command::new("python3")
            .args([
                "-c",
                "import json, pathlib; ns={}; \
                 exec(pathlib.Path('scripts/_v1_shared.py').read_text(), ns); \
                 print(json.dumps({int(k): v for k, v in ns['V1_KEY_TABLE'].items()}))",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR").replace("/daemon", ""))
            .output();
        let Ok(output) = output else {
            return; // no python3 on this box — the per-code test above still pins the conflicts
        };
        assert!(
            output.status.success(),
            "python3 check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let py: std::collections::HashMap<u32, String> =
            serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
                .expect("python table must be JSON");
        let rust = create_iohook_to_web_key_mapping();
        assert_eq!(
            py.len(),
            py.keys().collect::<std::collections::HashSet<_>>().len(),
            "python table has duplicate keys"
        );
        for (code, name) in &py {
            assert_eq!(
                rust.get(code),
                Some(name),
                "code {code}: python={name:?}, rust={:?} — the two V1→V2 paths diverge",
                rust.get(code)
            );
        }
        // The Rust table is built by repeated HashMap::insert, so a duplicate
        // key fails silently (last wins) — the bug that corrupted
        // 3597/3612/3640. Scan the source for re-inserted codes.
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/utils/old_pack_fixer.rs"
        ))
        .expect("read own source");
        let start = src.find("fn create_iohook_to_web_key_mapping").unwrap();
        // Bound the scan to the function body (up to its `mapping` return) so
        // test strings that mention `mapping.insert(` are not counted.
        let body_end = src[start..]
            .find("\n    mapping\n")
            .map(|i| start + i)
            .unwrap_or(src.len());
        let body = &src[start..body_end];
        let mut codes: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut rest = body;
        while let Some(i) = rest.find("mapping.insert(") {
            let after = &rest[i + "mapping.insert(".len()..];
            let len = after.chars().take_while(|c| c.is_ascii_digit()).count();
            let code = &after[..len];
            codes.entry(code).or_insert(0);
            *codes.get_mut(code).unwrap() += 1;
            rest = &after[code.len()..];
        }
        let dupes: Vec<_> = codes.into_iter().filter(|(_, n)| *n > 1).collect();
        assert!(
            dupes.is_empty(),
            "duplicate mapping.insert codes: {dupes:?}"
        );
    }

    /// Zero-padded code is a release.
    #[test]
    fn a_zero_padded_v1_code_is_a_release_not_a_duplicate_press() {
        assert_eq!(super::iohook_code_and_press("1"), Some((1, true)));
        assert_eq!(super::iohook_code_and_press("2"), Some((2, true)));
        assert_eq!(super::iohook_code_and_press("01"), Some((1, false)));
        assert_eq!(super::iohook_code_and_press("02"), Some((2, false)));

        assert_eq!(super::iohook_code_and_press("10"), Some((10, true)));
        assert_eq!(super::iohook_code_and_press("not a number"), None);
    }

    #[test]
    fn iohook_press_and_release_both_survive_conversion() {
        // inserting rather than appending would silently drop one.
        let dir = temp_dir("v1-press-release");
        let config_path = dir.join("config.json");

        for name in ["1.wav", "2.wav"] {
            write_tone_wav(&dir.join(name), 44100, 2, 100);
        }

        std::fs::write(
            &config_path,
            r#"{
  "id": "custom-sound-pack-77777732",
  "name": "Test Pack",
  "key_define_type": "multi",
  "includes_numpad": false,
  "sound": "sound.wav",
  "defines": { "30": "1.wav", "48": "2.wav", "030": "1.wav", "048": "2.wav" }
}"#,
        )
        .expect("write v1 config");

        super::convert_v1_to_v2(
            config_path.to_str().unwrap(),
            config_path.to_str().unwrap(),
            Some(dir.to_str().unwrap()),
        )
        .expect("conversion must succeed");

        let converted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read converted"))
                .expect("converted config must be valid JSON");
        let definitions = converted["definitions"].as_object().expect("definitions");

        for key in ["KeyA", "KeyB"] {
            if definitions.contains_key(key) {
                let timing = definitions[key]["timing"].as_array().expect("timing array");
                assert_eq!(
                    timing.len(),
                    2,
                    "{} must carry press and release: {:?}",
                    key,
                    timing
                );
            }
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sorakey-converter-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Write a synthetic sine-tone WAV so the mixed-rate tests need no binary
    /// fixtures checked into the repo.
    fn write_tone_wav(path: &std::path::Path, sample_rate: u32, channels: u16, duration_ms: u32) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        let frame_count = ((sample_rate as u64) * (duration_ms as u64)) / 1000;
        for frame in 0..frame_count {
            let t = (frame as f32) / (sample_rate as f32);
            let value = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
            let sample = (value * (i16::MAX as f32)) as i16;
            for _ in 0..channels {
                writer.write_sample(sample).expect("write sample");
            }
        }
        writer.finalize().expect("finalize wav");
    }

    /// Frames actually present in the written WAV, read back rather than
    /// inferred - this is what playback will see.
    fn wav_frames(path: &std::path::Path) -> (u32, u16, usize) {
        let reader = hound::WavReader::open(path).expect("open wav");
        let spec = reader.spec();
        let frames = (reader.len() as usize) / (spec.channels.max(1) as usize);
        (spec.sample_rate, spec.channels, frames)
    }

    /// The pitch bug in its smallest form: a clip whose native rate differs
    /// from the destination must come back with the frame count the
    /// destination rate implies. Returning the raw samples instead - the old
    /// behaviour - leaves half as many frames, which the concatenated file
    /// then plays at double speed, one octave up.
    #[test]
    fn a_lower_rate_clip_is_resampled_up_to_the_reference_rate() {
        let frames_at_22050 = 2205; // 100ms
        let samples = vec![0.0f32; frames_at_22050];

        let converted = convert_audio_format(&samples, 1, 22_050, 1, 44_100);

        let expected = 4410; // the same 100ms at 44100Hz
        let diff = (converted.len() as i64 - expected).unsigned_abs() as usize;
        assert!(
            diff <= 2,
            "expected ~{} frames at the reference rate, got {} (samples passed through unresampled?)",
            expected,
            converted.len()
        );
    }

    /// Channel conversion and rate conversion have to happen in the same pass;
    /// a mono clip landing in a stereo pack must end up with both the right
    /// interleaving and the right frame count.
    #[test]
    fn a_mono_lower_rate_clip_becomes_stereo_at_the_reference_rate() {
        let frames_at_22050 = 2205; // 100ms mono
        let samples = vec![0.25f32; frames_at_22050];

        let converted = convert_audio_format(&samples, 1, 22_050, 2, 44_100);

        let frames = converted.len() / 2;
        let expected_frames = 4410;
        let diff = (frames as i64 - expected_frames).unsigned_abs() as usize;
        assert!(
            diff <= 2,
            "expected ~{} stereo frames, got {} (len {})",
            expected_frames,
            frames,
            converted.len()
        );
        assert_eq!(
            converted.len() % 2,
            0,
            "a stereo buffer must hold whole frames"
        );
    }

    /// End to end through the concatenation path: the second file is sourced
    /// at half the reference rate, exactly the mixed-rate pack that produced
    /// the high-pitched nav keys. Its slice must occupy its real duration in
    /// the concatenated file, and the reported timing must match.
    #[test]
    fn a_mixed_rate_pack_concatenates_without_shrinking_the_odd_clip() {
        let dir = temp_dir("mixed-rate");
        write_tone_wav(&dir.join("letters.wav"), 44_100, 1, 100);
        write_tone_wav(&dir.join("delete.wav"), 22_050, 1, 100);

        let timing = concatenate_audio_files_with_timing(
            &["letters.wav".to_string(), "delete.wav".to_string()],
            dir.to_str().expect("utf8 dir"),
            "concatenated_audio.wav",
        )
        .expect("concatenation must succeed");

        let (offset, duration) = *timing.get("delete.wav").expect("delete.wav timing");
        assert!(
            (offset - 100.0).abs() < 1.0,
            "delete.wav should start after letters.wav's 100ms, got {:.2}ms",
            offset
        );
        assert!(
            (duration - 100.0).abs() < 1.0,
            "delete.wav is 100ms of audio; a reported {:.2}ms means it was written \
             unresampled and will play back pitched",
            duration
        );

        let (rate, channels, frames) = wav_frames(&dir.join("concatenated_audio.wav"));
        assert_eq!(rate, 44_100, "the reference rate sets the output rate");
        assert_eq!(channels, 1);
        let expected_frames = 4410 * 2; // 200ms total at the reference rate
        let diff = (frames as i64 - expected_frames).unsigned_abs() as usize;
        assert!(
            diff <= 4,
            "expected ~{} frames for 200ms at 44100Hz, got {}",
            expected_frames,
            frames
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The same path with a mono clip joining a stereo reference: frame count
    /// must stay honest once both the layout and the rate are converted.
    #[test]
    fn a_mono_clip_joining_a_stereo_pack_keeps_its_duration() {
        let dir = temp_dir("mono-into-stereo");
        write_tone_wav(&dir.join("letters.wav"), 44_100, 2, 100);
        write_tone_wav(&dir.join("delete.wav"), 22_050, 1, 100);

        let timing = concatenate_audio_files_with_timing(
            &["letters.wav".to_string(), "delete.wav".to_string()],
            dir.to_str().expect("utf8 dir"),
            "concatenated_audio.wav",
        )
        .expect("concatenation must succeed");

        let (offset, duration) = *timing.get("delete.wav").expect("delete.wav timing");
        assert!(
            (offset - 100.0).abs() < 1.0,
            "unexpected offset {:.2}ms",
            offset
        );
        assert!(
            (duration - 100.0).abs() < 1.0,
            "a mono 22050Hz clip in a stereo 44100Hz pack should still report 100ms, got {:.2}ms",
            duration
        );

        let (rate, channels, frames) = wav_frames(&dir.join("concatenated_audio.wav"));
        assert_eq!(rate, 44_100);
        assert_eq!(channels, 2, "the reference layout sets the output layout");
        let expected_frames = 4410 * 2;
        let diff = (frames as i64 - expected_frames).unsigned_abs() as usize;
        assert!(
            diff <= 4,
            "expected ~{} stereo frames, got {}",
            expected_frames,
            frames
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The destructive case: a pack that already ships its own
    /// `concatenated_audio.wav`. Conversion overwrites that name, so the
    /// author's audio has to survive somewhere before the write happens.
    #[test]
    fn an_existing_audio_file_is_preserved_before_being_overwritten() {
        let dir = temp_dir("backup");
        let audio = dir.join("concatenated_audio.wav");
        std::fs::write(&audio, b"the pack author's own audio").expect("write");

        let backup = back_up_existing_file(&audio)
            .expect("backing up an existing file must succeed")
            .expect("an existing file must produce a backup");

        assert_eq!(
            std::fs::read(&backup).expect("backup readable"),
            b"the pack author's own audio",
            "the original bytes must survive verbatim"
        );
        assert!(
            audio.exists(),
            "the original stays in place as conversion input"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Re-running a conversion must not let the second run's backup overwrite
    /// the first - that first `.bak` is the pristine original, and the later
    /// one is merely previously generated output.
    #[test]
    fn a_second_backup_does_not_clobber_the_first() {
        let dir = temp_dir("backup-twice");
        let audio = dir.join("concatenated_audio.wav");

        std::fs::write(&audio, b"pristine original").expect("write");
        let first = back_up_existing_file(&audio)
            .expect("first backup")
            .expect("first backup");

        std::fs::write(&audio, b"generated output").expect("write");
        let second = back_up_existing_file(&audio)
            .expect("second backup")
            .expect("second backup");

        assert_ne!(first, second, "the second backup must take a distinct path");
        assert_eq!(
            std::fs::read(&first).expect("first backup readable"),
            b"pristine original",
            "the pristine original must not be overwritten by a later run"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The common case - a pack with no such file yet - must not be reported
    /// as a backup, or callers would log a rescue that never happened.
    #[test]
    fn a_missing_file_needs_no_backup() {
        let dir = temp_dir("backup-absent");

        let result = back_up_existing_file(&dir.join("concatenated_audio.wav"))
            .expect("an absent file is not an error");
        assert!(result.is_none(), "nothing to back up means no backup path");

        std::fs::remove_dir_all(&dir).ok();
    }
}
