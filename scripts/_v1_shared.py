"""Shared V1 tables for import + admin converter."""
import copy
import sys

V1_KEY_TABLE = {
    "1": "Escape",
    "2": "Digit1", "3": "Digit2", "4": "Digit3", "5": "Digit4",
    "6": "Digit5", "7": "Digit6", "8": "Digit7", "9": "Digit8",
    "10": "Digit9", "11": "Digit0",
    "12": "Minus", "13": "Equal",
    "14": "Backspace",
    "15": "Tab",
    "16": "KeyQ", "17": "KeyW", "18": "KeyE", "19": "KeyR",
    "20": "KeyT", "21": "KeyY", "22": "KeyU", "23": "KeyI",
    "24": "KeyO", "25": "KeyP",
    "26": "BracketLeft", "27": "BracketRight",
    "28": "Enter",
    "29": "ControlLeft",
    "30": "KeyA", "31": "KeyS", "32": "KeyD", "33": "KeyF",
    "34": "KeyG", "35": "KeyH", "36": "KeyJ", "37": "KeyK",
    "38": "KeyL",
    "39": "Semicolon", "40": "Quote", "41": "Backquote",
    "42": "ShiftLeft",
    "43": "Backslash",
    "44": "KeyZ", "45": "KeyX", "46": "KeyC", "47": "KeyV",
    "48": "KeyB", "49": "KeyN", "50": "KeyM",
    "51": "Comma", "52": "Period", "53": "Slash",
    "54": "ShiftRight",
    "55": "NumpadMultiply",
    "56": "AltLeft",
    "57": "Space",
    "58": "CapsLock",
    "59": "F1", "60": "F2", "61": "F3", "62": "F4",
    "63": "F5", "64": "F6", "65": "F7", "66": "F8",
    "67": "F9", "68": "F10",
    "69": "NumLock", "70": "ScrollLock",
    "71": "Numpad7", "72": "Numpad8", "73": "Numpad9",
    "74": "NumpadSubtract",
    "75": "Numpad4", "76": "Numpad5", "77": "Numpad6",
    "78": "NumpadAdd",
    "79": "Numpad1", "80": "Numpad2", "81": "Numpad3",
    "82": "Numpad0",
    "83": "NumpadDecimal",
    "87": "F11", "88": "F12",
    "91": "F13", "92": "F14", "93": "F15",
    "99": "F16", "100": "F17", "101": "F18", "102": "F19",
    "103": "F20", "104": "F21", "105": "F22", "106": "F23",
    "107": "F24",
    "112": "Convert", "115": "Lang1", "119": "Lang2",
    "121": "KanaMode", "123": "HiraganaKatakana",
    "125": "IntlYen", "126": "NumpadComma",
    # Alternative 0x0Exx codes some V1 packs use instead of the main block.
    # 3597/3612 are main-only (their correct names live below); 3613 and
    # 3640 conflict with the main block so they live here (last-wins). Keep in
    # EXACT lockstep with daemon/src/utils/old_pack_fixer.rs — the parity
    # test compares the two final tables and fails on any divergence.
    "3613": "ControlRight",
    "3639": "Numpad7",
    "3640": "AltRight",
    "3653": "Numpad9",
    "3655": "NumpadAdd",
    "3657": "Numpad4",
    "3663": "Numpad5",
    "3665": "Numpad6",
    "3666": "Numpad1",
    "3667": "Numpad2",
    "58444": "Clear",
    "58470": "IntlBackslash",
    "3637": "NumpadDivide",
    "3612": "NumpadEnter",
    "3597": "ControlRight",
    "3645": "NumpadEquals",
    "3675": "NumpadDecimal",
    "3676": "Numpad0",
    "57399": "PrintScreen",
    "57415": "Home",
    "57416": "ArrowUp",
    "57417": "PageUp",
    "57419": "ArrowLeft",
    "57421": "ArrowRight",
    "57423": "End",
    "57424": "ArrowDown",
    "57425": "PageDown",
    "57426": "Insert",
    "57427": "Delete",
    "57400": "AltRight",
    "57435": "MetaLeft",
    "57436": "MetaRight",
    "57437": "ContextMenu",
    "57438": "Power",
    "57439": "Sleep",
    "57443": "WakeUp",
    "57360": "MediaTrackPrevious",
    "57369": "MediaTrackNext",
    "57376": "AudioVolumeMute",
    "57377": "LaunchApp2",
    "57378": "MediaPlayPause",
    "57380": "MediaStop",
    "57390": "AudioVolumeDown",
    "57392": "AudioVolumeUp",
    "57394": "BrowserHome",
    "57404": "LaunchApp1",
    "57444": "LaunchApp3",
    "57445": "BrowserSearch",
    "57446": "BrowserFavorites",
    "57447": "BrowserRefresh",
    "57448": "BrowserStop",
    "57449": "BrowserForward",
    "57450": "BrowserBack",
    "57452": "LaunchMail",
    "57453": "MediaSelect",
}

SMART_DONOR = {
    "MetaLeft": ["CapsLock", "ControlLeft", "AltLeft", "KeyA"],
    "MetaRight": ["CapsLock", "ControlLeft", "AltLeft", "KeyA"],
    "ContextMenu": ["CapsLock", "ControlLeft", "KeyA"],
    "AltRight": ["AltLeft", "ControlLeft", "KeyA"],
    "ControlRight": ["ControlLeft", "ShiftLeft", "KeyA"],
    "PrintScreen": ["Escape", "F12", "KeyA"],
    "ScrollLock": ["Escape", "CapsLock", "KeyA"],
    "Pause": ["Escape", "CapsLock", "KeyA"],
    "Insert": ["Backspace", "Delete", "KeyA"],
    "Delete": ["Backspace", "Insert", "KeyA"],
    "Home": ["PageUp", "ArrowUp", "Backspace", "KeyA"],
    "End": ["PageDown", "ArrowDown", "Enter", "KeyA"],
    "PageUp": ["Home", "ArrowUp", "KeyA"],
    "PageDown": ["End", "ArrowDown", "KeyA"],
    "ArrowUp": ["ArrowDown", "Space", "KeyA"],
    "ArrowDown": ["ArrowUp", "Space", "KeyA"],
    "ArrowLeft": ["ArrowRight", "Space", "KeyA"],
    "ArrowRight": ["ArrowLeft", "Space", "KeyA"],
    "Power": ["Escape", "KeyA"],
    "Sleep": ["Escape", "KeyA"],
    "WakeUp": ["Escape", "KeyA"],
    "NumLock": ["CapsLock", "KeyA"],
    "Clear": ["CapsLock", "KeyA"],
    "Numpad0": ["KeyA"], "Numpad1": ["KeyA"], "Numpad2": ["KeyA"],
    "Numpad3": ["KeyA"], "Numpad4": ["KeyA"], "Numpad5": ["KeyA"],
    "Numpad6": ["KeyA"], "Numpad7": ["KeyA"], "Numpad8": ["KeyA"],
    "Numpad9": ["KeyA"], "NumpadDecimal": ["KeyA"], "NumpadAdd": ["KeyA"],
    "NumpadSubtract": ["KeyA"], "NumpadMultiply": ["KeyA"],
    "NumpadDivide": ["KeyA"], "NumpadEnter": ["KeyA"],
    "NumpadEquals": ["KeyA"], "NumpadComma": ["KeyA"],
    "AudioVolumeMute": ["Space", "Enter", "KeyA"],
    "AudioVolumeDown": ["Space", "Enter", "KeyA"],
    "AudioVolumeUp": ["Space", "Enter", "KeyA"],
    "MediaTrackPrevious": ["Space", "Enter", "KeyA"],
    "MediaTrackNext": ["Space", "Enter", "KeyA"],
    "MediaPlayPause": ["Space", "Enter", "KeyA"],
    "MediaStop": ["Space", "Enter", "KeyA"],
    "MediaSelect": ["Space", "Enter", "KeyA"],
    "LaunchApp1": ["Space", "Enter", "KeyA"],
    "LaunchApp2": ["Space", "Enter", "KeyA"],
    "LaunchApp3": ["Space", "Enter", "KeyA"],
    "LaunchMail": ["Space", "Enter", "KeyA"],
    "BrowserHome": ["Space", "Enter", "KeyA"],
    "BrowserSearch": ["Space", "Enter", "KeyA"],
    "BrowserFavorites": ["Space", "Enter", "KeyA"],
    "BrowserRefresh": ["Space", "Enter", "KeyA"],
    "BrowserStop": ["Space", "Enter", "KeyA"],
    "BrowserForward": ["Space", "Enter", "KeyA"],
    "BrowserBack": ["Space", "Enter", "KeyA"],
}

def _fill_missing_keys(definitions):
    for missing, donors in SMART_DONOR.items():
        if missing in definitions:
            continue
        for donor in donors:
            if donor in definitions:
                definitions[missing] = copy.deepcopy(definitions[donor])
                break


# Detached-run result channel, shared by the import + export pickers so the
# two copies can't diverge again. When launched via scripts/sorakey-detached
# (immune to plugin-reload SIGTERM), stdout goes to a log nobody reads —
# the single result line ALSO goes to --result-file, which SoraService.qml
# polls (and resumes polling after its own restart).
RESULT_FILE = None


def emit(line):
    """Result line to stdout AND to the result file (if any)."""
    print(line, flush=True)
    if RESULT_FILE:
        try:
            with open(RESULT_FILE, "w") as f:
                f.write(line + "\n")
        except Exception as e:
            # A failed write would leave QML polling forever for a line that
            # never lands; stderr at least reaches the sidecar .log.
            print(f"RESULT-FILE-WRITE-FAILED ({e}): {line}", file=sys.stderr, flush=True)


def take_result_file_argv():
    """Strip --result-file PATH from argv (any position)."""
    global RESULT_FILE
    args = []
    skip_next = False
    for i in range(len(sys.argv)):
        if skip_next:
            skip_next = False
            continue
        if sys.argv[i] == "--result-file" and i + 1 < len(sys.argv):
            RESULT_FILE = sys.argv[i + 1]
            skip_next = True
        else:
            args.append(sys.argv[i])
    sys.argv = args
