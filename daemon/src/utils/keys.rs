/// W3C key names for the standard key range, shared by every code→name table
/// in the daemon. Codes are Linux input-event codes, which are numerically
/// identical to the V1/IOHook codes in this range — one table so the runtime
/// listener and the V1→V2 converter cannot drift (they did, see the
/// 3597/3612/3613/3640 corruption: B3/B4).
///
/// Above code 88 the two keyspaces disagree (evdev KEY_RIGHTALT=100 vs IOHook
/// 57400, evdev KEY_MACRO=112 vs IOHook "Convert"), so each consumer owns its
/// extended entries.
pub static KEY_MAP: &[(u16, &str)] = &[
    (1, "Escape"),
    (2, "Digit1"),
    (3, "Digit2"),
    (4, "Digit3"),
    (5, "Digit4"),
    (6, "Digit5"),
    (7, "Digit6"),
    (8, "Digit7"),
    (9, "Digit8"),
    (10, "Digit9"),
    (11, "Digit0"),
    (12, "Minus"),
    (13, "Equal"),
    (14, "Backspace"),
    (15, "Tab"),
    (16, "KeyQ"),
    (17, "KeyW"),
    (18, "KeyE"),
    (19, "KeyR"),
    (20, "KeyT"),
    (21, "KeyY"),
    (22, "KeyU"),
    (23, "KeyI"),
    (24, "KeyO"),
    (25, "KeyP"),
    (26, "BracketLeft"),
    (27, "BracketRight"),
    (28, "Enter"),
    (29, "ControlLeft"),
    (30, "KeyA"),
    (31, "KeyS"),
    (32, "KeyD"),
    (33, "KeyF"),
    (34, "KeyG"),
    (35, "KeyH"),
    (36, "KeyJ"),
    (37, "KeyK"),
    (38, "KeyL"),
    (39, "Semicolon"),
    (40, "Quote"),
    (41, "Backquote"),
    (42, "ShiftLeft"),
    (43, "Backslash"),
    (44, "KeyZ"),
    (45, "KeyX"),
    (46, "KeyC"),
    (47, "KeyV"),
    (48, "KeyB"),
    (49, "KeyN"),
    (50, "KeyM"),
    (51, "Comma"),
    (52, "Period"),
    (53, "Slash"),
    (54, "ShiftRight"),
    (55, "NumpadMultiply"),
    (56, "AltLeft"),
    (57, "Space"),
    (58, "CapsLock"),
    (59, "F1"),
    (60, "F2"),
    (61, "F3"),
    (62, "F4"),
    (63, "F5"),
    (64, "F6"),
    (65, "F7"),
    (66, "F8"),
    (67, "F9"),
    (68, "F10"),
    (69, "NumLock"),
    (70, "ScrollLock"),
    (71, "Numpad7"),
    (72, "Numpad8"),
    (73, "Numpad9"),
    (74, "NumpadSubtract"),
    (75, "Numpad4"),
    (76, "Numpad5"),
    (77, "Numpad6"),
    (78, "NumpadAdd"),
    (79, "Numpad1"),
    (80, "Numpad2"),
    (81, "Numpad3"),
    (82, "Numpad0"),
    (83, "NumpadDecimal"),
    // 84 is reserved in linux/input-event-codes.h (no KEY_* defined between
    // KEY_KPDOT 83 and KEY_ZENKAKUHANKAKU 85) — deliberately unmapped.
    (85, "ZenkakuHankaku"),
    (86, "IntlBackslash"),
    (87, "F11"),
    (88, "F12"),
];

/// W3C name for a Linux input-event code in the shared range.
pub fn w3c_name(code: u16) -> Option<&'static str> {
    KEY_MAP.iter().find(|&&(c, _)| c == code).map(|&(_, n)| n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A code mapped twice silently shadows the first entry — the exact
    /// failure mode that corrupted B3/B4. Smallest check that fails if one
    /// comes back.
    #[test]
    fn no_code_is_mapped_twice() {
        let mut seen: HashMap<u16, &str> = HashMap::new();
        for &(code, name) in KEY_MAP {
            if let Some(first) = seen.insert(code, name) {
                panic!("code {} mapped to both {} and {}", code, first, name);
            }
        }
    }

    /// Spot-check that the shared range stays pinned to Linux input-event
    /// codes (KEY_ESC=1, KEY_A=30, KEY_SPACE=57, KEY_F12=88).
    #[test]
    fn shared_range_matches_linux_input_codes() {
        assert_eq!(w3c_name(1), Some("Escape"));
        assert_eq!(w3c_name(30), Some("KeyA"));
        assert_eq!(w3c_name(57), Some("Space"));
        assert_eq!(w3c_name(88), Some("F12"));
        assert_eq!(w3c_name(0), None);
        assert_eq!(w3c_name(89), None);
    }
}
