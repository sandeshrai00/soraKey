//! The only place `config.json` is written — use `apply` to mutate.

use crate::state::settings::AppConfig;
use std::sync::{Mutex, OnceLock};

/// Authoritative config — loaded once, then held in memory.
static AUTHORITY: OnceLock<Mutex<AppConfig>> = OnceLock::new();

fn authority() -> &'static Mutex<AppConfig> {
    AUTHORITY.get_or_init(|| Mutex::new(AppConfig::load()))
}

/// Snapshot of current config (read-only).
pub fn current() -> AppConfig {
    // recover from poisoned lock instead of crashing
    match authority().lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Apply mutation to config and persist if changed.
pub fn apply(mutate: impl FnOnce(&mut AppConfig)) -> bool {
    let mut guard = match authority().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let before = guard.clone();
    mutate(&mut guard);

    let changed = !guard.data_equals(&before);
    if changed {
        if let Err(e) = guard.save() {
            crate::always_eprint!("❌ [config] Failed to save config: {}", e);
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Serialize tests using the real authority.
    static REAL_AUTHORITY_TESTS: Mutex<()> = Mutex::new(());

    fn lock_real_authority() -> std::sync::MutexGuard<'static, ()> {
        match REAL_AUTHORITY_TESTS.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    struct RestoreGuard {
        path: PathBuf,
        before: Option<Vec<u8>>,
        original: AppConfig,
    }
    impl Drop for RestoreGuard {
        fn drop(&mut self) {
            if let Some(bytes) = &self.before {
                let _ = std::fs::write(&self.path, bytes);
            } else {
                let _ = std::fs::remove_file(&self.path);
            }
            // restore in-memory authority as well
            match authority().lock() {
                Ok(mut g) => *g = self.original.clone(),
                Err(p) => *p.into_inner() = self.original.clone(),
            }
        }
    }

    /// Real writers on different threads all survive.
    #[test]
    fn real_writers_on_different_threads_all_survive() {
        use std::sync::{Arc, Barrier};

        let _serialised = lock_real_authority();
        let original = current();
        let path = crate::state::folders::data::config_json();
        let before = std::fs::read(&path).ok();
        let _guard = RestoreGuard {
            path,
            before,
            original: original.clone(),
        };

        let threads = 8;
        let iterations = 40;
        let barrier = Arc::new(Barrier::new(threads));

        let handles: Vec<_> = (0..threads)
            .map(|id| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..iterations {
                        match id {
                            0 => apply(|config| {
                                config.volume = 0.5;
                            }),
                            1 => apply(|config| {
                                config.volume = 0.25;
                            }),
                            2 => apply(|config| {
                                config.enable_sound = false;
                            }),
                            3 => apply(|config| {
                                config
                                    .per_pack_volume
                                    .insert("keyboard/test".to_string(), 0.5);
                            }),
                            6 => apply(|config| {
                                config.keyboard_soundpack = "keyboard/test".to_string();
                            }),
                            _ => apply(|config| {
                                config.selected_audio_device = Some("test".to_string());
                            }),
                        };
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("no writer thread may panic");
        }

        let final_config = current();
        apply(|config| {
            *config = original;
        });
        // At least one writer's effect must be present; exact values race but no panic proves compose
        assert!(final_config.volume == 0.5 || final_config.volume == 0.25);
        // These fields each have a single writer, so their final value is deterministic.
        assert!(
            !final_config.enable_sound,
            "mute writer's effect must survive"
        );
    }

    /// Reader never sees a truncated document.
    #[test]
    fn a_concurrent_reader_never_observes_a_partial_document() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

        let _serialised = lock_real_authority();
        let original = current();
        let path = crate::state::folders::data::config_json();
        let before = std::fs::read(&path).ok();
        let _guard = RestoreGuard {
            path: path.clone(),
            before,
            original: original.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));

        let reader = {
            let stop = Arc::clone(&stop);
            let path = path.clone();
            std::thread::spawn(move || {
                let mut partial_reads = 0usize;
                let mut successful_reads = 0usize;
                while !stop.load(AtomicOrdering::Relaxed) {
                    // missing is okay — only broken is a failure
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if crate::state::settings::parse_lenient(&contents).is_ok() {
                            successful_reads += 1;
                        } else {
                            partial_reads += 1;
                        }
                    }
                }
                (successful_reads, partial_reads)
            })
        };

        for round in 0..200 {
            apply(|config| {
                config.volume = 0.3 + (round as f32 % 10.0) * 0.01;
            });
        }

        stop.store(true, AtomicOrdering::Relaxed);
        let (successful_reads, partial_reads) = reader.join().expect("reader must not panic");

        apply(|config| {
            *config = original;
        });

        assert!(
            successful_reads > 0,
            "the reader must actually have observed the file"
        );
        assert_eq!(
            partial_reads, 0,
            "a half-written config was observed {partial_reads} times - \
             the write is not atomic"
        );
    }
}
