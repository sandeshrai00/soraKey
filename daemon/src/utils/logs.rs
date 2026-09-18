//! In-memory ring buffer for recent log lines. Shown in Settings and exported on demand.
//! Memory only, fixed size. Never stores key identities.

use std::collections::VecDeque;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const CAPACITY: usize = 2000;
#[cfg(test)]
pub const VIEWER_LINES: usize = 100;

static BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
#[cfg(test)]
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn buffer() -> &'static Mutex<VecDeque<String>> {
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

/// Current time as HH:MM:SS.mmm.
fn timestamp_now() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}

#[cfg(test)]
fn format_timestamp(at: chrono::DateTime<chrono::Local>) -> String {
    at.format("%H:%M:%S%.3f").to_string()
}

/// Append a line, dropping the oldest when full.
pub fn push(line: &str) {
    let Ok(mut buffer) = buffer().lock() else {
        return;
    };

    for part in line.split('\n') {
        if buffer.len() >= CAPACITY {
            buffer.pop_front();
        }
        buffer.push_back(format!(
            "{} {}",
            timestamp_now(),
            part.trim_end_matches('\r')
        ));
    }

    drop(buffer);
    #[cfg(test)]
    GENERATION.fetch_add(1, Ordering::Release);
}

/// All lines, oldest first.
pub fn snapshot() -> Vec<String> {
    let Ok(buffer) = buffer().lock() else {
        return Vec::new();
    };
    buffer.iter().cloned().collect()
}

#[cfg(test)]
pub fn recent(count: usize) -> Vec<String> {
    let Ok(buffer) = buffer().lock() else {
        return Vec::new();
    };
    let skip = buffer.len().saturating_sub(count);
    buffer.iter().skip(skip).cloned().collect()
}

#[cfg(test)]
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub fn len() -> usize {
    buffer().lock().map(|buffer| buffer.len()).unwrap_or(0)
}

fn current_user_name() -> Option<String> {
    for var in ["USERNAME", "USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(var) {
            let trimmed = value.trim();
            if trimmed.len() >= 3 {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Replace account name in paths with `[username]`.
pub fn mask_user_paths(line: &str) -> String {
    let Some(user) = current_user_name() else {
        return line.to_string();
    };
    mask_name_in_paths(line, &user)
}

fn mask_name_in_paths(line: &str, user: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    let needle = user.to_ascii_lowercase();

    loop {
        let haystack = rest.to_ascii_lowercase();
        let Some(at) = haystack.find(&needle) else {
            break;
        };

        let before_ok = at == 0 || {
            let prev = rest[..at].chars().next_back();
            matches!(prev, Some('/') | Some('\\'))
        };
        let after = at + needle.len();
        let after_ok =
            after == rest.len() || matches!(rest[after..].chars().next(), Some('/') | Some('\\'));

        out.push_str(&rest[..at]);
        if before_ok && after_ok {
            out.push_str("[username]");
        } else {
            out.push_str(&rest[at..after]);
        }
        rest = &rest[after..];
    }

    out.push_str(rest);
    out
}

pub fn export_header() -> String {
    format!(
        "Sorakey log export\n\
         App version: {}\n\
         OS: {} ({})\n\
         Exported at: {}\n\
         Verbose logging: {}\n\
         Lines captured: {} (buffer holds up to {})\n\
         {}\n",
        crate::utils::version::APP_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        "off",
        len(),
        CAPACITY,
        "-".repeat(60)
    )
}

/// Key identities must never leave the machine in an export: the evdev
/// listener logs `Sending key press: {code}` for the first keys, which
/// reveals typing content. Redact those lines during export only — the
/// live ring buffer keeps them for local debugging.
fn redact_key_identities(line: &str) -> String {
    if line.contains("Sending key press:") {
        match line.find("Sending key press:") {
            Some(at) => format!("{}Sending key press: [redacted]", &line[..at]),
            None => "[redacted key press line]".to_string(),
        }
    } else {
        line.to_string()
    }
}

pub fn export_contents() -> String {
    let mut out = mask_user_paths(&export_header());
    for line in snapshot() {
        out.push_str(&mask_user_paths(&redact_key_identities(&line)));
        out.push('\n');
    }
    out
}

#[cfg(test)]
fn export_file_name(at: chrono::DateTime<chrono::Local>) -> String {
    format!("sorakey-log-{}.txt", at.format("%Y%m%d-%H%M%S"))
}

#[cfg(test)]
pub fn export_to_file() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join("sorakey-logs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {}", dir.display(), e))?;

    let path = dir.join(export_file_name(chrono::Local::now()));
    std::fs::write(&path, export_contents())
        .map_err(|e| format!("Could not write {}: {}", path.display(), e))?;

    Ok(path)
}

#[cfg(test)]
pub fn buffer_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        buffer().lock().unwrap().clear();
    }

    fn our_indices(marker: &str) -> Vec<usize> {
        snapshot()
            .iter()
            .filter_map(|line| line.split(marker).nth(1)?.trim().parse().ok())
            .collect()
    }

    #[test]
    fn lines_are_kept_in_order_with_a_timestamp() {
        let _guard = super::buffer_test_guard();
        reset();

        push("ordertest 0");
        push("ordertest 1");

        let ours = our_indices("ordertest ");
        assert_eq!(ours, vec![0, 1]);
        let first = snapshot()
            .into_iter()
            .find(|l| l.contains("ordertest 0"))
            .expect("pushed line must be present");
        assert!(
            first.chars().take(2).all(|c| c.is_ascii_digit()),
            "{}",
            first
        );
    }

    #[test]
    fn the_buffer_rotates_and_never_grows_past_capacity() {
        let _guard = super::buffer_test_guard();
        reset();

        for i in 0..CAPACITY + 50 {
            push(&format!("rotcap {}", i));
        }

        assert_eq!(len(), CAPACITY);
        let ours = our_indices("rotcap ");
        assert!(ours.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(*ours.last().unwrap(), CAPACITY + 49);
        assert!(*ours.first().unwrap() >= 50);
    }

    #[test]
    fn recent_returns_the_tail_oldest_first() {
        let _guard = super::buffer_test_guard();
        reset();

        for i in 0..10 {
            push(&format!("tailtest {}", i));
        }

        let tail = recent(6);
        let ours: Vec<usize> = tail
            .iter()
            .filter_map(|line| line.split("tailtest ").nth(1)?.trim().parse().ok())
            .collect();
        assert!(
            ours.len() >= 3,
            "tail must contain our newest lines: {:?}",
            tail
        );
        assert!(ours.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(*ours.last().unwrap(), 9);
    }

    #[test]
    fn recent_asking_for_more_than_exists_returns_everything() {
        let _guard = super::buffer_test_guard();
        reset();

        push("loneline 0");
        let tail = recent(VIEWER_LINES);
        assert!(tail.iter().any(|l| l.contains("loneline 0")), "{:?}", tail);
    }

    #[test]
    fn a_multi_line_message_becomes_multiple_entries() {
        let _guard = super::buffer_test_guard();
        reset();

        push("multiline-alpha\nmultiline-beta");

        let lines = snapshot();
        let ours: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("multiline-"))
            .collect();

        assert_eq!(ours.len(), 2, "{:?}", lines);
        assert!(ours[0].ends_with("multiline-alpha"), "{}", ours[0]);
        assert!(ours[1].ends_with("multiline-beta"), "{}", ours[1]);
    }

    #[test]
    fn timestamps_are_zero_padded_hms_with_milliseconds() {
        use chrono::TimeZone;

        let at = chrono::Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        assert_eq!(format_timestamp(at), "03:04:05.000");

        let at = chrono::Local
            .with_ymd_and_hms(2026, 1, 2, 13, 45, 59)
            .unwrap();
        assert_eq!(format_timestamp(at), "13:45:59.000");
    }

    #[test]
    fn every_push_moves_the_generation() {
        let _guard = super::buffer_test_guard();
        reset();

        let before = generation();
        push("something");
        assert!(generation() > before);
    }

    #[test]
    fn the_export_carries_the_header_and_every_line() {
        let _guard = super::buffer_test_guard();
        reset();

        let marker = "export-header-line";
        for i in 0..5 {
            push(&format!("{} {}", marker, i));
        }

        let contents = export_contents();

        assert!(contents.contains("Sorakey log export"), "{}", contents);
        assert!(
            contents.contains(&format!(
                "App version: {}",
                crate::utils::version::APP_VERSION
            )),
            "{}",
            contents
        );
        assert!(contents.contains(std::env::consts::OS), "{}", contents);
        assert!(contents.contains("Verbose logging: off"), "{}", contents);

        let reported = contents
            .lines()
            .find_map(|line| line.strip_prefix("Lines captured: "))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("{}", contents));
        assert!(reported >= 5);

        for i in 0..5 {
            assert!(
                contents.contains(&format!("{} {}", marker, i)),
                "missing {} {}",
                marker,
                i
            );
        }
    }

    #[test]
    fn the_export_hides_the_account_name_in_paths() {
        let _guard = super::buffer_test_guard();
        reset();

        let Some(user) = super::current_user_name() else {
            return;
        };

        push(&format!(
            r"soundpack_dir: C:\Users\{}\AppData\Local\Sorakey",
            user
        ));
        push(&format!(
            "config: /home/{}/.config/sorakey/config.json",
            user
        ));

        let contents = export_contents();

        assert!(!contents.contains(&user), "{}", contents);
        assert!(contents.contains("[username]"), "{}", contents);
        assert!(contents.contains(r"\AppData\Local\Sorakey"), "{}", contents);
        assert!(
            contents.contains("/.config/sorakey/config.json"),
            "{}",
            contents
        );
    }

    #[test]
    fn masking_replaces_whole_path_components_only() {
        let masked = super::mask_name_in_paths(r"C:\Users\ada\ada-theme\adamant.ogg", "ada");

        assert!(masked.contains(r"C:\Users\[username]\"), "{}", masked);
        assert!(masked.contains("adamant.ogg"), "{}", masked);
        assert!(masked.contains("ada-theme"), "{}", masked);
    }

    #[test]
    fn masking_ignores_case_and_covers_both_path_separators() {
        let masked =
            super::mask_name_in_paths(r"C:\Users\AroCodes\x and /home/arocodes/y", "arocodes");

        assert!(!masked.to_lowercase().contains("arocodes"), "{}", masked);
        assert_eq!(masked, r"C:\Users\[username]\x and /home/[username]/y");
    }

    #[test]
    fn a_name_at_the_end_of_a_path_is_still_masked() {
        let masked = super::mask_name_in_paths(r"home dir: C:\Users\arocodes", "arocodes");
        assert_eq!(masked, r"home dir: C:\Users\[username]");
    }

    #[test]
    fn the_export_contains_more_than_the_viewer_shows() {
        let _guard = super::buffer_test_guard();
        reset();

        let total = VIEWER_LINES + 250;
        for i in 0..total {
            push(&format!("event {}", i));
        }

        assert_eq!(recent(VIEWER_LINES).len(), VIEWER_LINES);

        let contents = export_contents();
        assert!(contents.contains("event 0"));
        assert!(contents.contains(&format!("event {}", total - 1)));
        let captured: usize = contents
            .lines()
            .find_map(|l| {
                l.split("Lines captured: ")
                    .nth(1)?
                    .split(' ')
                    .next()?
                    .parse()
                    .ok()
            })
            .expect("header must report count");
        assert!(captured >= total, "header said {}", captured);
    }

    #[test]
    fn export_file_names_are_timestamped_and_do_not_collide() {
        use chrono::TimeZone;

        let at = chrono::Local
            .with_ymd_and_hms(2026, 8, 4, 15, 30, 45)
            .unwrap();
        assert_eq!(export_file_name(at), "sorakey-log-20260804-153045.txt");

        let later = chrono::Local
            .with_ymd_and_hms(2026, 8, 4, 15, 30, 46)
            .unwrap();
        assert_ne!(export_file_name(at), export_file_name(later));
    }

    #[test]
    #[ignore = "diagnostic: prints a sample export"]
    fn show_a_real_export() {
        let _guard = super::buffer_test_guard();
        reset();

        push("🐛 Debug logging enabled");
        push("🚀 Initializing Sorakey...");
        push("📂 App root (from exe): D:\\sorakey\\target\\release");
        push("🎧 Audio engine thread started");
        push("🎮 Starting Raw Input worker process...");

        let path = export_to_file().expect("export must succeed");
        println!("--- exported to: {} ---", path.display());
        println!("{}", std::fs::read_to_string(&path).unwrap());
        println!("--- end ---");
    }

    #[test]
    fn exporting_writes_a_readable_file_containing_the_buffer() {
        let _guard = super::buffer_test_guard();
        reset();

        push("a line that must survive the round trip");

        let path = export_to_file().expect("export must succeed");
        let written = std::fs::read_to_string(&path).expect("exported file must be readable");

        assert!(written.contains("Sorakey log export"));
        assert!(written.contains("a line that must survive the round trip"));

        let _ = std::fs::remove_file(&path);
    }
}
