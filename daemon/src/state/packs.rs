use crate::state::folders;
use crate::utils::{files, json_files, pack_info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// Serializes compound cache updates (load → mutate → save) across threads
/// (control socket, engine load worker). The file write itself is atomic
/// (temp + rename), but without this two threads can load the same snapshot
/// and the second save silently drops the first thread's mutation.
static CACHE_LOCK: Mutex<()> = Mutex::new(());

/// Hold the returned guard across a whole load → mutate → save sequence.
/// The lock is non-reentrant: while it is held, use `load_locked` /
/// `save_locked` (the public `load` / `save` would deadlock on it).
pub(crate) fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
    match CACHE_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn default_config_version() -> u32 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundpackOptions {
    #[serde(default = "default_recommended_volume")]
    pub recommended_volume: f32,
    #[serde(default = "default_random_pitch")]
    pub random_pitch: bool,
}

fn default_recommended_volume() -> f32 {
    1.0
}

fn default_random_pitch() -> bool {
    false
}

impl Default for SoundpackOptions {
    fn default() -> Self {
        Self {
            recommended_volume: 1.0,
            random_pitch: false,
        }
    }
}

// V2 key definition
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyDefinition {
    pub timing: Vec<[f32; 2]>, // Array of [start_ms, end_ms] pairs
    #[serde(default)]
    pub audio_file: Option<String>, // For "multi" definition method
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundPack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub config_version: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub audio_file: Option<String>, // Used only in "single" definition_method
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub created_at: Option<String>, // ISO-8601 string
    pub definition_method: String, // "single" or "multi"
    #[serde(default)]
    pub options: SoundpackOptions,
    #[serde(default = "default_config_version")]
    pub config_version_num: u32, // Internal config version number
    pub definitions: HashMap<String, KeyDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundpackMetadata {
    pub id: String, // Original ID from soundpack config (should not be modified)
    pub name: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub tags: Vec<String>,
    pub icon: Option<String>,
    pub folder_path: String, // Relative path from soundpacks directory (e.g., "keyboard/Super Paper Mario Talk")
    pub last_modified: u64,
    // Validation fields
    pub config_version: Option<u32>,
    pub is_valid_v2: bool,
    pub validation_status: String,
    // Error tracking
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundpackCache {
    pub soundpacks: HashMap<String, SoundpackMetadata>,
    pub last_scan: u64,
    pub cache_version: u32,
    #[serde(default)]
    pub count: usize,
}

impl SoundpackCache {
    fn cache_file() -> String {
        folders::data::soundpack_cache_json()
            .to_string_lossy()
            .to_string()
    }

    pub fn load() -> Self {
        let _guard = cache_lock();
        Self::load_locked()
    }

    pub(crate) fn load_locked() -> Self {
        let cache_file = Self::cache_file();
        let mut cache = match json_files::load_json_from_file::<SoundpackCache>(
            std::path::Path::new(&cache_file),
        ) {
            Ok(cache) => {
                crate::always_print!(
                    "📦 Loaded soundpack metadata cache with {} entries",
                    cache.soundpacks.len()
                );
                cache
            }
            Err(e) => {
                crate::always_eprint!("⚠️  Failed to load cache file: {}", e);
                Self::new()
            }
        };

        if cache.soundpacks.is_empty() {
            crate::always_print!("🔄 Cache is empty, refreshing from soundpack directories...");
            cache.refresh_from_directory();
            cache.save_locked();
        }

        cache
    }
    pub fn new() -> Self {
        Self {
            soundpacks: HashMap::new(),
            last_scan: 0,
            cache_version: 4,
            count: 0,
        }
    }

    pub(crate) fn save_locked(&self) {
        let cache_file = Self::cache_file();

        if let Some(parent) = Path::new(&cache_file).parent() {
            if let Err(e) = files::ensure_directory_exists(parent) {
                crate::always_eprint!("⚠️  Failed to create cache directory: {}", e);
                return;
            }
        }

        match json_files::save_json_to_file_atomically(self, std::path::Path::new(&cache_file)) {
            Ok(_) => crate::always_print!(
                "💾 Saved soundpack metadata cache with {} entries",
                self.soundpacks.len()
            ),
            Err(e) => crate::always_eprint!("⚠️  Failed to save metadata cache: {}", e),
        }
    }

    pub fn add_soundpack(&mut self, metadata: SoundpackMetadata) {
        self.soundpacks.insert(metadata.id.clone(), metadata);
    }
    pub fn refresh_from_directory(&mut self) {
        crate::always_print!("📂 Scanning soundpacks directories...");

        self.soundpacks.clear();

        let soundpacks_dir = folders::soundpacks::get_builtin_soundpacks_dir()
            .to_string_lossy()
            .to_string();
        crate::always_print!("📂 Scanning soundpacks in: {}", soundpacks_dir);
        self.scan_soundpack_type(&soundpacks_dir, "keyboard");

        self.update_count();

        self.last_scan = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        crate::always_print!("📦 Loaded {} soundpacks metadata", self.soundpacks.len());
    }

    pub fn update_count(&mut self) {
        self.count = self.soundpacks.len();

        crate::always_print!("📊 Updated count: {} soundpacks", self.count);
    }

    fn scan_soundpack_type(&mut self, soundpacks_dir: &str, soundpack_type: &str) {
        let type_dir = std::path::Path::new(soundpacks_dir).join(soundpack_type);
        crate::always_print!(
            "📂 [CACHE DEBUG] Scanning {} soundpacks in: {}",
            soundpack_type,
            type_dir.display()
        );
        crate::always_print!("   Directory exists: {}", type_dir.exists());

        if type_dir.exists() {
            crate::always_print!("✅ Directory found, reading entries...");

            if let Ok(entries) = std::fs::read_dir(&type_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    if let Some(soundpack_name) = entry.file_name().to_str() {
                        let full_soundpack_id = format!("{}/{}", soundpack_type, soundpack_name);
                        crate::always_print!(
                            "🔍 [CACHE DEBUG] Processing soundpack: {}",
                            full_soundpack_id
                        );

                        match pack_info::load_soundpack_metadata(&full_soundpack_id) {
                            Ok(metadata) => {
                                crate::always_print!(
                                    "✅ [CACHE DEBUG] Successfully loaded metadata for: {}",
                                    full_soundpack_id
                                );
                                self.soundpacks.insert(full_soundpack_id, metadata);
                            }
                            Err(e) => {
                                crate::always_print!(
                                    "❌ [CACHE DEBUG] Failed to load {} metadata for {}: {}",
                                    soundpack_type,
                                    soundpack_name,
                                    e
                                );
                                self.insert_error_metadata(&full_soundpack_id, soundpack_name, e);
                            }
                        }
                    }
                }
            } else {
                crate::always_print!(
                    "❌ [CACHE DEBUG] Failed to read directory: {}",
                    type_dir.display()
                );
            }
        } else {
            crate::always_print!(
                "⚠️ [CACHE DEBUG] Directory does not exist: {}",
                type_dir.display()
            );
            crate::always_print!("   Expected at: {}", type_dir.display());
            if let Some(parent) = type_dir.parent() {
                crate::always_print!("   Parent directory: {}", parent.display());
                crate::always_print!("   Parent exists: {}", parent.exists());
            }
        }
    }

    fn insert_error_metadata(
        &mut self,
        full_soundpack_id: &str,
        soundpack_name: &str,
        error: String,
    ) {
        let error_metadata = SoundpackMetadata {
            id: full_soundpack_id.to_string(),
            name: format!("Error: {}", soundpack_name),
            author: None,
            description: Some(format!("Failed to load: {}", error)),
            version: "unknown".to_string(),
            tags: vec!["error".to_string()],
            icon: None,
            folder_path: full_soundpack_id.to_string(),
            last_modified: 0,
            config_version: None,
            is_valid_v2: false,
            validation_status: "error".to_string(),
            last_error: Some(error),
        };
        self.soundpacks
            .insert(full_soundpack_id.to_string(), error_metadata);
    }
}
