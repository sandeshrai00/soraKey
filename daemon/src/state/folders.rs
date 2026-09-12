//! Fixed layout under `~/.local/share/sorakey` — data + soundpacks.

use std::path::PathBuf;
use std::sync::OnceLock;

fn data_dir() -> PathBuf {
    match directories::BaseDirs::new() {
        Some(b) => b.data_dir().join("sorakey"),
        // No HOME (service sandbox without env): the old "." fallback put
        // the socket, lock and configs in whatever the CWD happened to be —
        // possibly shared — so use the sticky-bit temp dir instead.
        None => std::env::temp_dir().join("sorakey"),
    }
}

/// Writable state directory: `~/.local/share/sorakey/data`.
pub fn get_writable_data_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| data_dir().join("data"))
}

/// Consent note: written by sora-keyboard-access.sh on every successful
/// approval (`<uid> <iso-timestamp>`). Every removal path revokes it —
/// panel uninstall purges/moves the whole tree, the orphan self-clean
/// deletes the file — so remove+reinstall always re-asks, even if the OS
/// grant silently survived.
pub fn consent_stamp() -> PathBuf {
    data_dir().join("keyboard-granted")
}

pub mod data {
    use super::get_writable_data_dir;
    use std::path::PathBuf;

    pub fn config_json() -> PathBuf {
        get_writable_data_dir().join("config.json")
    }

    pub fn soundpack_cache_json() -> PathBuf {
        get_writable_data_dir().join("soundpack_cache.json")
    }
}

pub mod soundpacks {
    use super::data_dir;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    pub fn get_builtin_soundpacks_dir() -> PathBuf {
        static DIR: OnceLock<PathBuf> = OnceLock::new();
        DIR.get_or_init(|| data_dir().join("soundpacks")).clone()
    }

    pub fn ensure_soundpack_directories() -> std::io::Result<()> {
        std::fs::create_dir_all(get_builtin_soundpacks_dir().join("keyboard"))?;
        Ok(())
    }

    /// Preinstalled pack ids (`keyboard/<name>`) from the sora-build.sh
    /// stamp (`~/.local/share/sorakey/.bundled-packs`, one basename per
    /// line). Same sanitization as the writer: plain single-level dir
    /// names only, so a hand-edited stamp can't inject paths.
    pub fn bundled_pack_ids() -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        let stamp = super::data_dir().join(".bundled-packs");
        let Ok(content) = std::fs::read_to_string(&stamp) else {
            return out;
        };
        for line in content.lines() {
            let id = line.trim();
            if id.is_empty() || id.contains('/') || id.contains("..") || id.starts_with('-') {
                continue;
            }
            out.insert(format!("keyboard/{id}"));
        }
        out
    }

    /// Directory for a soundpack id.
    pub fn soundpack_dir(soundpack_id: &str) -> String {
        let sanitized = soundpack_id.replace('\\', "/");
        let parts: Vec<&str> = sanitized
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".." && !p.contains('\0'))
            .collect();
        let join = |base: &Path| -> PathBuf {
            parts
                .iter()
                .fold(base.to_path_buf(), |p, part| p.join(part))
        };
        let base = get_builtin_soundpacks_dir();
        let joined = join(Path::new(&base));
        // stay inside base dir
        if !joined.starts_with(&*base) {
            return base
                .join("keyboard")
                .join("invalid")
                .to_string_lossy()
                .to_string();
        }
        joined.to_string_lossy().to_string()
    }

    pub fn config_json(soundpack_id: &str) -> String {
        Path::new(&soundpack_dir(soundpack_id))
            .join("config.json")
            .to_string_lossy()
            .to_string()
    }

    /// Canonicalized pack directory, or `None` when it escapes the
    /// soundpacks root (symlink attack) or doesn't exist. Fail closed:
    /// callers deny the load, mirroring `delete_pack`.
    pub fn contained_dir(soundpack_id: &str) -> Option<PathBuf> {
        let base = get_builtin_soundpacks_dir();
        let canon_base = base.canonicalize().ok()?;
        let canon = Path::new(&soundpack_dir(soundpack_id))
            .canonicalize()
            .ok()?;
        canon.starts_with(&canon_base).then_some(canon)
    }

    /// Canonicalized file path guaranteed under an already-contained `dir`,
    /// or `None` for absolute/parent-traversal names and symlink escapes.
    pub fn contained_file(dir: &Path, rel: &str) -> Option<PathBuf> {
        let clean = rel.trim_start_matches("./").replace('\\', "/");
        if clean.is_empty()
            || clean.contains("..")
            || clean.contains('\0')
            || clean.starts_with('/')
        {
            return None;
        }
        let canon_dir = dir.canonicalize().ok()?;
        let canon = canon_dir.join(&clean).canonicalize().ok()?;
        canon.starts_with(&canon_dir).then_some(canon)
    }
}
