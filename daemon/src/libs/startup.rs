use crossbeam_channel::Sender;

pub fn start_input_capture(keyboard_tx: Sender<String>, hotkey_tx: Sender<String>) {
    #[cfg(target_os = "linux")]
    {
        crate::always_print!("🎮 Starting evdev keyboard listener...");
        crate::libs::keyboard::start_evdev_keyboard_listener(keyboard_tx, hotkey_tx);
    }
}
