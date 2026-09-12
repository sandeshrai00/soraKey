use std::sync::{Arc, Mutex};
use std::thread;

#[cfg(target_os = "linux")]
use crossbeam_channel::Sender;

/// Modifier counts shared across all device threads so a chord split over
/// two keyboards (e.g. Ctrl on kb1 + M on kb2) still fires the hotkey.
/// Counts (not bools) so two devices holding Ctrl at once release cleanly.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct SharedMods {
    ctrl_held: u32,
    alt_held: u32,
}

#[cfg(target_os = "linux")]
fn is_keyboard_device(device: &evdev::Device) -> bool {
    use evdev::KeyCode;
    let Some(keys) = device.supported_keys() else {
        return false;
    };
    // Anchor keys: any full keyboard has at least one of these. Letters,
    // Enter/Space/Backspace/Tab, digits, F-keys and the numpad block are
    // all accepted so A-less numpads/macropads stay visible. Power/media
    // keys (KEY_POWER etc.) are deliberately absent so those devices
    // stay excluded.
    const ANCHORS: &[KeyCode] = &[
        KeyCode::KEY_A,
        KeyCode::KEY_Q,
        KeyCode::KEY_Z,
        KeyCode::KEY_M,
        KeyCode::KEY_ENTER,
        KeyCode::KEY_SPACE,
        KeyCode::KEY_BACKSPACE,
        KeyCode::KEY_TAB,
        KeyCode::KEY_ESC,
        KeyCode::KEY_1,
        KeyCode::KEY_0,
        KeyCode::KEY_F1,
        KeyCode::KEY_F12,
        KeyCode::KEY_NUMLOCK,
        KeyCode::KEY_KP0,
        KeyCode::KEY_KP1,
        KeyCode::KEY_KP7,
        KeyCode::KEY_KPENTER,
        KeyCode::KEY_KPSLASH,
        KeyCode::KEY_KPDOT,
    ];
    if ANCHORS.iter().any(|k| keys.contains(*k)) {
        return true;
    }
    // Fallback for exotic macropads that only send e.g. F13+ or media keys:
    // EV_KEY capability (supported_keys is Some) plus a keyboard-ish name.
    let name = device.name().unwrap_or("").to_lowercase();
    name.contains("keyboard")
        || name.contains("keypad")
        || name.contains("numpad")
        || name.contains("macropad")
        || name.contains("kbd")
}

#[cfg(target_os = "linux")]
pub fn start_evdev_keyboard_listener(keyboard_tx: Sender<String>, hotkey_tx: Sender<String>) {
    crate::always_print!("🔍 [evdev] start_evdev_keyboard_listener() called - spawning thread");
    thread::spawn(move || {
        // One blocking thread per keyboard: fetch_events blocks until the
        // kernel delivers an event, so key-to-channel latency is ~0.
        // A supervisor below re-scans every 10s: unplugged devices error
        // out (their thread exits and is pruned), replugged/new keyboards
        // are picked up without a daemon restart. One entry per device
        // path, so a device is never double-held.
        let spawn_one = |path: std::path::PathBuf,
                         mut device: evdev::Device,
                         keyboard_tx: Sender<String>,
                         hotkey_tx: Sender<String>,
                         shared: Arc<Mutex<SharedMods>>|
         -> std::thread::JoinHandle<()> {
            thread::spawn(move || {
                use evdev::EventType;

                let mut local_ctrl = false;
                let mut local_alt = false;
                let mut event_count = 0;
                let mut first_event_logged = false;

                loop {
                    let mut events = match device.fetch_events() {
                        Ok(e) => e,
                        Err(e) => {
                            crate::always_eprint!(
                                "⚠️ [evdev] Error on {}: {} - device thread exiting (supervisor will rescan)",
                                path.display(),
                                e
                            );
                            break;
                        }
                    };
                    for event in events.by_ref() {
                        if event.event_type() != EventType::KEY {
                            continue;
                        }
                        event_count += 1;
                        if !first_event_logged {
                            crate::always_print!("✅ [evdev] First keyboard event detected!");
                            first_event_logged = true;
                        }

                        let key_value = event.value();
                        let key_code = map_evdev_keycode(event.code());
                        if key_code.is_empty() {
                            continue;
                        }

                        if key_value == 1 || key_value == 2 {
                            let is_repeat = key_value == 2;
                            match key_code {
                                "ControlLeft" | "ControlRight" => {
                                    if !is_repeat && !local_ctrl {
                                        local_ctrl = true;
                                        if let Ok(mut s) = shared.lock() {
                                            s.ctrl_held = s.ctrl_held.saturating_add(1);
                                        }
                                    }
                                }
                                "AltLeft" | "AltRight" => {
                                    if !is_repeat && !local_alt {
                                        local_alt = true;
                                        if let Ok(mut s) = shared.lock() {
                                            s.alt_held = s.alt_held.saturating_add(1);
                                        }
                                    }
                                }
                                "KeyM" => {
                                    // Hotkey fires on the initial edge only:
                                    // autorepeat of a held M must not
                                    // toggle mute repeatedly.
                                    if !is_repeat {
                                        let hot = shared
                                            .lock()
                                            .map(|s| s.ctrl_held > 0 && s.alt_held > 0)
                                            .unwrap_or(false);
                                        if hot {
                                            crate::always_print!(
                                                "🔥 [evdev] Hotkey detected: Ctrl+Alt+M - Toggling global sound"
                                            );
                                            let _ = hotkey_tx.send("TOGGLE_SOUND".to_string());
                                            continue;
                                        }
                                    }
                                }
                                _ => {}
                            }

                            if event_count <= 5 {
                                crate::always_print!("🔍 [evdev] Sending key press: {}", key_code);
                            }
                            let _ = keyboard_tx.send(key_code.to_string());
                        } else if key_value == 0 {
                            match key_code {
                                "ControlLeft" | "ControlRight" => {
                                    if local_ctrl {
                                        local_ctrl = false;
                                        if let Ok(mut s) = shared.lock() {
                                            s.ctrl_held = s.ctrl_held.saturating_sub(1);
                                        }
                                    }
                                }
                                "AltLeft" | "AltRight" => {
                                    if local_alt {
                                        local_alt = false;
                                        if let Ok(mut s) = shared.lock() {
                                            s.alt_held = s.alt_held.saturating_sub(1);
                                        }
                                    }
                                }
                                _ => {}
                            }

                            let _ = keyboard_tx.send(format!("UP:{}", key_code));
                        }
                    }
                }
            })
        };

        let shared: Arc<Mutex<SharedMods>> = Arc::new(Mutex::new(SharedMods::default()));

        let mut live: std::collections::HashMap<std::path::PathBuf, std::thread::JoinHandle<()>> =
            std::collections::HashMap::new();

        crate::always_print!("🔍 [evdev] Enumerating input devices...");
        // Retry loop: a fresh install often can't open /dev/input/event*
        // yet. Previously we returned here and the daemon stayed "running"
        // but forever silent. Now we report health and retry, so a later
        // permission grant or a late device plug-in recovers without a
        // daemon restart.
        let mut attempt: u32 = 0;
        loop {
            let devices: Vec<_> = evdev::enumerate().collect();
            let device_count = devices.len();
            if device_count == 0 {
                attempt += 1;
                crate::state::status::set_input_keyboards(0);
                crate::state::status::set_input_error(Some(
                    "no_input_devices: cannot open /dev/input/event*".to_string(),
                ));
                if attempt == 1 {
                    crate::always_eprint!(
                        "❌ [evdev] No devices found - cannot access /dev/input/event* devices"
                    );
                    crate::always_eprint!("🔁 [evdev] Retrying every 5s until devices appear");
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
            crate::always_print!("🔍 [evdev] Found {} total input devices", device_count);

            let mut keyboards: Vec<(std::path::PathBuf, evdev::Device)> = Vec::new();
            {
                for (path, device) in devices {
                    if live.contains_key(&path) {
                        continue;
                    }
                    if is_keyboard_device(&device) {
                        crate::always_print!(
                            "🔍 [evdev] Found keyboard device: {:?} - {}",
                            path.display(),
                            device.name().unwrap_or("Unknown")
                        );
                        keyboards.push((path, device));
                    }
                }
            }

            if keyboards.is_empty() && live.is_empty() {
                crate::state::status::set_input_keyboards(0);
                crate::state::status::set_input_error(Some(format!(
                    "no_keyboards: saw {device_count} input device(s) but none usable"
                )));
                crate::always_eprint!(
                    "❌ [evdev] No keyboard devices found among the {} input devices!",
                    device_count
                );
                crate::always_eprint!("🔁 [evdev] Retrying in 5s");
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }

            for (path, device) in keyboards {
                let p = path.clone();
                live.insert(
                    path,
                    spawn_one(
                        p,
                        device,
                        keyboard_tx.clone(),
                        hotkey_tx.clone(),
                        Arc::clone(&shared),
                    ),
                );
            }
            if !live.is_empty() {
                crate::state::status::set_input_keyboards(live.len());
                crate::state::status::set_input_error(None);
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }

        crate::always_print!(
            "✅ [evdev] Holding {} keyboard device(s) - supervisor active",
            live.len()
        );

        // Supervisor: prune dead threads, pick up new/replugged keyboards.
        loop {
            live.retain(|path, h| {
                if h.is_finished() {
                    crate::always_eprint!("🔌 [evdev] {} gone - will rescan", path.display());
                    return false;
                }
                true
            });
            for (path, device) in evdev::enumerate() {
                if live.contains_key(&path) {
                    continue;
                }
                if is_keyboard_device(&device) {
                    crate::always_print!(
                        "🔌 [evdev] New keyboard: {:?} - {}",
                        path.display(),
                        device.name().unwrap_or("Unknown")
                    );
                    let p = path.clone();
                    live.insert(
                        path,
                        spawn_one(
                            p,
                            device,
                            keyboard_tx.clone(),
                            hotkey_tx.clone(),
                            Arc::clone(&shared),
                        ),
                    );
                }
            }
            crate::state::status::set_input_keyboards(live.len());
            if live.is_empty() {
                crate::state::status::set_input_error(Some(
                    "no_keyboards: all keyboards disconnected".to_string(),
                ));
            } else {
                crate::state::status::set_input_error(None);
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

/// W3C name for a raw evdev key code; "" for keys the daemon does not sound.
/// The standard range (1-88) comes from the shared table so runtime key names
/// cannot drift from what the V1→V2 converter emits. Above 88 the evdev and
/// IOHook keyspaces diverge, so the evdev-specific extended codes live here.
#[cfg(target_os = "linux")]
fn map_evdev_keycode(code: u16) -> &'static str {
    if let Some(name) = crate::utils::keys::w3c_name(code) {
        return name;
    }

    use evdev::KeyCode;
    match KeyCode(code) {
        KeyCode::KEY_RIGHTCTRL => "ControlRight",
        KeyCode::KEY_RIGHTALT => "AltRight",
        KeyCode::KEY_LEFTMETA => "MetaLeft",
        KeyCode::KEY_RIGHTMETA => "MetaRight",

        KeyCode::KEY_UP => "ArrowUp",
        KeyCode::KEY_DOWN => "ArrowDown",
        KeyCode::KEY_LEFT => "ArrowLeft",
        KeyCode::KEY_RIGHT => "ArrowRight",

        KeyCode::KEY_INSERT => "Insert",
        KeyCode::KEY_DELETE => "Delete",
        KeyCode::KEY_HOME => "Home",
        KeyCode::KEY_END => "End",
        KeyCode::KEY_PAGEUP => "PageUp",
        KeyCode::KEY_PAGEDOWN => "PageDown",

        _ => "",
    }
}
