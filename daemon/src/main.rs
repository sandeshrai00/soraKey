//! sorakey daemon — mechanical keyboard sounds.
//! Forked from MechvibesDX audio core; control via Unix socket plus Ctrl+Alt+M mute.

mod commands;
mod libs;
mod state;
mod utils;

use crossbeam_channel::unbounded;

fn main() {
    // `sorakey ctl '<json>'` - client mode, exits after one request.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("ctl") {
        let request = args.get(2).map(String::as_str).unwrap_or("{}");
        std::process::exit(commands::ctl_client(request));
    }
    // `sorakey key <Code> [up]` - fire-and-forget keystroke for notifiers.
    if args.get(1).map(String::as_str) == Some("key") {
        let code = args.get(2).map(String::as_str).unwrap_or("");
        let down = args.get(3).map(String::as_str) != Some("up");
        std::process::exit(commands::key_client(code, down));
    }

    // Only one instance — two would double-play every keystroke.
    let _lock = match acquire_lock() {
        Some(f) => f,
        None => {
            eprintln!("sorakey: already running");
            std::process::exit(1);
        }
    };

    if let Err(e) = state::folders::soundpacks::ensure_soundpack_directories() {
        always_eprint!("⚠️  sorakey: could not create soundpack dirs: {e}");
    }

    let (keyboard_tx, keyboard_rx) = unbounded::<String>();
    let (hotkey_tx, hotkey_rx) = unbounded::<String>();

    let engine = libs::player::spawn_engine(keyboard_rx, hotkey_rx);

    if let Some(path) = commands::serve(engine) {
        always_print!("🔌 sorakey control socket: {}", path.display());
    } else {
        eprintln!("sorakey: control socket bind failed — exiting so systemd restarts the daemon");
        std::process::exit(1);
    }

    libs::startup::start_input_capture(keyboard_tx, hotkey_tx);
    always_print!("✅ sorakey ready. Ctrl+Alt+M mutes, Ctrl+C exits.");

    std::thread::park();
}

/// Exclusive lock file — kernel releases it on exit or crash.
fn acquire_lock() -> Option<std::fs::File> {
    let path = std::env::var("XDG_RUNTIME_DIR")
        .map(|dir| std::path::PathBuf::from(dir).join("sorakey.lock"))
        .unwrap_or_else(|_| {
            // Same reasoning as folders::data_dir: never lock in CWD.
            std::env::temp_dir().join("sorakey.lock")
        });
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false) // lock file — the flock is the content, never empty it
        .open(&path)
        .ok()?;
    let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
    let acquired = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0 };
    if acquired { Some(file) } else { None }
}
