/// Path and file system utility functions
use crate::state::folders;
use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Sibling temp path unique per call (pid + nanos + process counter), in the
/// same directory as `target` so a later rename stays on one filesystem.
/// Returns `Err` if anything — regular file AND symlink — is already planted
/// at the chosen path, so callers never write through an attacker link.
pub fn unique_sibling_temp(target: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = target.as_os_str().to_os_string();
    name.push(format!(".{}.{}.{}.tmp", std::process::id(), nanos, n));
    let tmp = std::path::PathBuf::from(name);
    if fs::symlink_metadata(&tmp).is_ok() {
        return Err(format!(
            "temp path already exists, refusing to follow: {}",
            tmp.display()
        ));
    }
    Ok(tmp)
}

/// Exclusively-created (O_EXCL, 0600) empty file at a unique sibling temp.
/// `create_new` fails when the path exists — including a planted symlink —
/// closing the check-then-use race the pre-check above cannot.
pub fn create_sibling_temp(
    target: &std::path::Path,
) -> Result<(std::path::PathBuf, fs::File), String> {
    let tmp = unique_sibling_temp(target)?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| {
            format!(
                "Failed to exclusively create temp file '{}': {}",
                tmp.display(),
                e
            )
        })?;
    Ok((tmp, file))
}

/// Get absolute path for soundpacks directory (built-in soundpacks)
pub fn get_soundpacks_dir_absolute() -> String {
    folders::soundpacks::get_builtin_soundpacks_dir()
        .to_string_lossy()
        .to_string()
}

/// Create directory recursively if it doesn't exist
pub fn ensure_directory_exists(path: impl AsRef<std::path::Path>) -> Result<(), String> {
    let path_ref = path.as_ref();
    fs::create_dir_all(path_ref)
        .map_err(|e| format!("Failed to create directory '{}': {}", path_ref.display(), e))
}

/// Read file contents as string
pub fn read_file_contents(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))
}
