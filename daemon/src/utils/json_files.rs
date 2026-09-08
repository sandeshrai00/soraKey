use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Load JSON from file.
pub fn load_json_from_file<T>(file_path: &Path) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path.display(), e))?;

    serde_json::from_str::<T>(&contents)
        .map_err(|e| format!("Failed to parse JSON from '{}': {}", file_path.display(), e))
}

/// Save JSON atomically — write to sibling temp file then rename.
pub fn save_json_to_file_atomically<T>(data: &T, file_path: &Path) -> Result<(), String>
where
    T: Serialize,
{
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory '{}': {}", parent.display(), e))?;
    }

    let contents = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize data: {}", e))?;

    // Exclusive, owner-only temp: no pid-guessable name, no symlink follow.
    let (temp_path, mut temp_file) = super::files::create_sibling_temp(file_path)?;
    use std::io::Write;
    temp_file.write_all(contents.as_bytes()).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("Failed to write file '{}': {}", temp_path.display(), e)
    })?;
    temp_file.sync_all().map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("Failed to sync file '{}': {}", temp_path.display(), e)
    })?;
    drop(temp_file);

    fs::rename(&temp_path, file_path).map_err(|e| {
        // clean up temp on failure
        let _ = fs::remove_file(&temp_path);
        format!("Failed to replace file '{}': {}", file_path.display(), e)
    })
}
