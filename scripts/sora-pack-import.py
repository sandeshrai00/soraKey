#!/usr/bin/env python3
"""Import a soundpack from a ZIP.

Handles V1→V2 conversion; preserves paths relative to config.json.
Installs to ~/.local/share/sorakey/soundpacks/keyboard/{id}/.
"""

import json
import os
import re
import sys
import uuid
import zipfile
import subprocess
import tempfile

# CANONICAL flag — NOT the dead alias sys.dontwritebytecode (a silent
# no-op on Python 3.14, verified: canonical stays False after the alias
# is set). Stops CPython from compiling the _v1_shared import below into
# scripts/__pycache__ inside the watched plugin dir. That write used to
# make the shell reload the plugin on first tap — bar blink + teardown
# murder of the picker. Systemic twin: PYTHONPYCACHEPREFIX in
# sorakey-detached redirects ALL bytecode out of the plugin dir.
sys.dont_write_bytecode = True

try:
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gtk, Gio, GLib
except Exception:
    Gtk = None

SOUNDPACKS = os.path.expanduser("~/.local/share/sorakey/soundpacks")

# zip bomb guard
MAX_PACK_SIZE = 20 * 1024 * 1024

# non-zip extensions for dialog filter
NON_ZIP_EXTS = (".7z", ".rar", ".tar", ".gz", ".bz2", ".xz")

# Detached-run support: result channel lives in _v1_shared (single copy).
# See _v1_shared.emit / take_result_file_argv.
def emit(line):
    _mod.emit(line)


def take_result_file_argv():
    _mod.take_result_file_argv()


def _short(value, limit=50):
    """Short one-line error text."""
    s = " ".join(str(value).split())
    if len(s) > limit:
        s = s[:limit - 1] + "…"
    return s or "unknown error"


import importlib.util as _ilu
import pathlib as _pl
# This exec does NOT write scripts/__pycache__ into the watched plugin dir:
# sys.dont_write_bytecode (top of file) suppresses it, and the detached
# launcher redirects any stray bytecode to ~/.cache via PYTHONPYCACHEPREFIX.
_spec = _ilu.spec_from_file_location("_v1_shared", str(_pl.Path(__file__).with_name("_v1_shared.py")))
_mod = _ilu.module_from_spec(_spec); _spec.loader.exec_module(_mod)
V1_KEY_TABLE = _mod.V1_KEY_TABLE
SMART_DONOR = _mod.SMART_DONOR
_fill_missing_keys = _mod._fill_missing_keys


def is_v1_config(cfg):
    """V1 uses `defines` + `sound`, no V2 fields."""
    if not isinstance(cfg, dict):
        return False
    if "definition_method" in cfg or "definitions" in cfg or "defs" in cfg:
        return False
    if "defines" in cfg:
        return True
    return False


def is_v1_multi(defines):
    """V1 multi-method — string file paths per key."""
    if not defines:
        return False
    string_values = sum(1 for v in defines.values() if isinstance(v, str) and v)
    return string_values > sum(1 for v in defines.values() if isinstance(v, list))


def looks_auto_generated_id(s):
    """Check if id looks auto-generated."""
    if not s:
        return True
    s = str(s).strip()
    if s.startswith("imported-"):
        return True
    if s.startswith("custom-sound-pack-"):
        return True
    if re.fullmatch(r"[0-9][0-9_\-]*", s):
        return True
    return False


def derive_name_from_zip(zip_path):
    """Zip path -> (slug, label)."""
    base = os.path.basename(zip_path)
    if base.lower().endswith(".zip"):
        base = base[:-4]
    slug = re.sub(r"[^a-zA-Z0-9_-]+", "-", base).strip("-").lower()
    if not slug:
        slug = f"imported-{uuid.uuid4().hex[:8]}"
    label = re.sub(r"[-_]+", " ", base).strip()
    if not label:
        label = slug
    return slug, label


def get_audio_duration_ms(audio_path, zf=None, zip_prefix=None):
    """Audio duration in ms. ffprobe for most, wave for WAV."""
    try:
        if zf is not None and zip_prefix is not None:
            zip_path = zip_prefix + audio_path
            with zf.open(zip_path) as src:
                data = src.read()
        elif zf is not None:
            with zf.open(audio_path) as src:
                data = src.read()
        else:
            with open(audio_path, "rb") as f:
                data = f.read()
    except Exception:
        return None

    # check WAV
    is_wav = data[:4] == b'RIFF' and data[8:12] == b'WAVE'
    if is_wav:
        try:
            import wave as wave_module
            import io
            with wave_module.open(io.BytesIO(data)) as w:
                return w.getnframes() / w.getframerate() * 1000.0
        except Exception:
            pass

    # ffprobe for everything else
    tmp = None
    try:
        with tempfile.NamedTemporaryFile(suffix=os.path.splitext(audio_path)[1], delete=False) as f:
            f.write(data)
            tmp = f.name
        try:
            result = subprocess.run(
                ["ffprobe", "-v", "quiet", "-print_format", "json", "-show_format", tmp],
                capture_output=True, text=True, timeout=10
            )
        except Exception:
            # ffprobe missing — fallback to 100ms
            return None
    finally:
        if tmp:
            try:
                os.unlink(tmp)
            except Exception:
                pass

    try:
        d = json.loads(result.stdout)
        return float(d["format"]["duration"]) * 1000.0
    except Exception:
        return None


def _case_insensitive_match(filename, file_list):
    """Case-insensitive file lookup."""
    filename_lower = filename.lower()
    base_lower = os.path.basename(filename).lower()
    for f in file_list:
        if f.lower() == filename_lower:
            return f
        if os.path.basename(f).lower() == base_lower:
            return f
    return None




def convert_v1_to_v2(cfg, available_files, soundpack_dir=None, zf=None, zip_prefix=None):
    """Convert V1 config to V2."""
    defines = cfg.pop("defines", {})
    sound = cfg.pop("sound", None)

    definitions = {}

    if is_v1_multi(defines):
        per_key_files = {}
        for code, filename in defines.items():
            if not isinstance(filename, str) or not filename or filename.strip().lower() == "null":
                continue
            w3c_name = V1_KEY_TABLE.get(str(code))
            if not w3c_name:
                continue
            per_key_files[w3c_name] = filename.strip()

        unique_files = set(per_key_files.values())

        # Normalize all filenames to their case-matched ZIP names
        normalized_files = {}
        for fname in unique_files:
            matched = _case_insensitive_match(fname, available_files)
            if matched:
                normalized_files[fname] = matched

        per_key_files_normalized = {}
        for w3c_name, fname in per_key_files.items():
            per_key_files_normalized[w3c_name] = normalized_files.get(fname, fname)

        normalized_unique = set(per_key_files_normalized.values())
        chosen_audio = None
        for f in normalized_unique:
            if f in available_files:
                chosen_audio = f
                break
        if chosen_audio is None and sound:
            matched = _case_insensitive_match(sound, available_files)
            if matched:
                chosen_audio = matched

        if len(normalized_unique) == 1 and chosen_audio:
            duration_ms = 100.0
            dur = get_audio_duration_ms(chosen_audio, zf, zip_prefix)
            if dur and dur > 0:
                duration_ms = dur
            for w3c_name in per_key_files_normalized:
                definitions[w3c_name] = {"timing": [[0.0, duration_ms]]}
            definition_method = "single"
            audio_file = chosen_audio
        else:
            file_durations = {}
            for fname in normalized_unique:
                dur = get_audio_duration_ms(fname, zf, zip_prefix)
                if dur and dur > 0:
                    file_durations[fname] = dur
                else:
                    file_durations[fname] = 100.0

            for w3c_name, filename in per_key_files_normalized.items():
                dur = file_durations.get(filename, 100.0)
                definitions[w3c_name] = {
                    "timing": [[0.0, dur]],
                    "audio_file": filename,
                }
            definition_method = "multi"
            audio_file = None
    else:
        # single-method: sprite regions
        for code, timing in defines.items():
            w3c_name = V1_KEY_TABLE.get(str(code))
            if not w3c_name:
                continue
            if isinstance(timing, list) and len(timing) == 2 and all(
                isinstance(x, (int, float)) for x in timing
            ):
                start = float(timing[0])
                duration = float(timing[1])
                end = start + duration
                definitions[w3c_name] = {
                    "timing": [[start, end]]
                }
        definition_method = "single"
        audio_file = None
        if sound and sound in available_files:
            audio_file = sound
        elif sound:
            audio_file = sound

    _fill_missing_keys(definitions)

    new_cfg = {
        "id": cfg.get("id") or f"imported-{uuid.uuid4().hex[:8]}",
        "name": cfg.get("name") or "Imported Soundpack",
        "author": cfg.get("author") or cfg.get("m_author") or "Unknown",
        "config_version": "2",
        "definition_method": definition_method,
        "definitions": definitions,
        "soundpack_type": "Keyboard",
    }
    if audio_file:
        new_cfg["audio_file"] = audio_file
    if cfg.get("version"):
        new_cfg["version"] = cfg["version"]
    if cfg.get("description"):
        new_cfg["description"] = cfg["description"]
    if cfg.get("tags"):
        new_cfg["tags"] = cfg["tags"]
    return new_cfg


def find_config_in_zip(zf):
    """Find config.json in ZIP — shallowest wins."""
    candidates = []
    for name in zf.namelist():
        if name.endswith("/") or not name.endswith("config.json"):
            continue
        depth = name.count("/")
        candidates.append((depth, name))
    if not candidates:
        return None, None
    candidates.sort()
    _, path = candidates[0]
    return path, zf.read(path)


def list_files_in_zip(zf, config_path, strip_folder):
    """List files relative to config."""
    prefix = ""
    if strip_folder:
        top = config_path.split("/")[0]
        prefix = top + "/"
    out = []
    for name in zf.namelist():
        if name.endswith("/"):
            continue
        rel = name[len(prefix):] if prefix and name.startswith(prefix) else name
        if not rel:
            continue
        out.append(rel)
    return out


def strip_enclosing_folder(zf, config_path):
    """Check if ZIP has single enclosing folder."""
    names = [n for n in zf.namelist() if not n.endswith("/")]
    if not names:
        return False
    first_slash = names[0].find("/")
    if first_slash < 0:
        return False
    top = names[0][:first_slash]
    if not all(n.startswith(top + "/") for n in names):
        return False
    if not config_path.startswith(top + "/"):
        return False
    return True


def validate_v2(cfg):
    """Validate V2 config — None ok, else reason."""
    if not cfg.get("name"):
        return "no name"
    if not cfg.get("author"):
        return "no author"
    if not (cfg.get("definitions") or cfg.get("defs")):
        return "no key definitions"
    if cfg.get("definition_method") not in ("single", "multi"):
        return "definition_method must be single or multi"
    if cfg.get("definition_method") == "single" and not cfg.get("audio_file"):
        return "single method needs audio_file"
    return None


def import_zip(zip_path):
    try:
        zf = zipfile.ZipFile(zip_path, "r")
    except zipfile.BadZipFile:
        ext = os.path.splitext(zip_path)[1].lower()
        if ext in NON_ZIP_EXTS:
            return None, f"Not a ZIP — this is {ext}. Extract it, then compress the folder as .zip."
        return None, "Not a valid ZIP — file may be corrupted."
    except Exception as e:
        return None, f"Can't read file: {_short(e, 40)}"
    with zf:
        # zip bomb guard FIRST: total decompressed size from metadata alone,
        # before any entry is read into RAM or probed.
        try:
            total = sum(zf.getinfo(n).file_size for n in zf.namelist())
            if total > MAX_PACK_SIZE:
                return None, f"Pack too big: {total // (1024 * 1024)}MB (max {MAX_PACK_SIZE // (1024 * 1024)}MB)."
        except Exception:
            pass
        config_path, config_bytes = find_config_in_zip(zf)
        if not config_path:
            return None, "No config.json — not a soundpack."

        try:
            cfg = json.loads(config_bytes)
        except Exception:
            return None, "config.json damaged — re-download pack."

        strip_folder = strip_enclosing_folder(zf, config_path)
        available_files = list_files_in_zip(zf, config_path, strip_folder)
        if not available_files:
            return None, "ZIP is empty or damaged."

        zip_slug, zip_label = derive_name_from_zip(zip_path)
        if looks_auto_generated_id(cfg.get("id")):
            cfg["id"] = zip_slug
        if looks_auto_generated_id(cfg.get("name")) or not cfg.get("name"):
            cfg["name"] = zip_label
        cfg["id"] = re.sub(r"[^a-zA-Z0-9_-]+", "-", str(cfg["id"])).strip("-").lower()
        if not cfg["id"]:
            cfg["id"] = zip_slug

        soundpack_id = cfg["id"]
        install_dir = os.path.join(SOUNDPACKS, "keyboard", soundpack_id)

        zip_prefix = ""
        if strip_folder:
            zip_prefix = config_path.split("/")[0] + "/"

        if is_v1_config(cfg):
            cfg = convert_v1_to_v2(cfg, available_files, install_dir, zf, zip_prefix)
        else:
            err = validate_v2(cfg)
            if err:
                return None, f"Bad config: {err}"

        # validate audio exists before replacing install
        if cfg.get("definition_method") == "single":
            audio_rel = cfg.get("audio_file", "")
            if audio_rel and audio_rel not in available_files and not _case_insensitive_match(audio_rel, available_files):
                return None, f"Missing audio file: {audio_rel}"
        elif cfg.get("definition_method") == "multi":
            missing = sorted({d.get("audio_file") for d in cfg.get("definitions", {}).values() if d.get("audio_file") and d.get("audio_file") not in available_files and not _case_insensitive_match(d.get("audio_file"), available_files)})
            if missing:
                if len(missing) == 1:
                    return None, f"Missing audio: {missing[0]}"
                names = ", ".join(missing[:2])
                if len(missing) > 2:
                    names += f" +{len(missing) - 2} more"
                return None, f"Missing audio: {names}"

        import shutil, pathlib
        # private staging dir: mkdtemp is unique and mode 0700, so a
        # predictable path can't be pre-planted as a symlink.
        os.makedirs(os.path.dirname(install_dir), exist_ok=True)
        tmp_dir = tempfile.mkdtemp(prefix=os.path.basename(install_dir) + ".tmp.", dir=os.path.dirname(install_dir))
        skipped_traversal = []
        try:
            prefix = ""
            if strip_folder:
                top = config_path.split("/")[0]
                prefix = top + "/"
            for name in zf.namelist():
                if name.endswith("/"):
                    continue
                rel = name[len(prefix):] if prefix and name.startswith(prefix) else name
                if not rel:
                    continue
                # guard against traversal
                if rel.startswith("/") or ".." in pathlib.PurePosixPath(rel).parts or "\\" in rel:
                    skipped_traversal.append(name)
                    continue
                target = os.path.join(tmp_dir, rel)
                # keep inside tmp_dir (trailing sep: /tmp/pack must not match /tmp/pack-evil)
                if not os.path.abspath(target).startswith(os.path.abspath(tmp_dir) + os.sep):
                    skipped_traversal.append(name)
                    continue
                os.makedirs(os.path.dirname(target), exist_ok=True)
                import shutil as _sh
                with zf.open(name) as src, open(target, "wb") as dst:
                    _sh.copyfileobj(src, dst, length=64*1024)
            if skipped_traversal:
                # Install proceeds without them; say what was dropped instead
                # of silently shipping a pack minus files (stderr: sidecar log).
                print(f"WARNING: skipped {len(skipped_traversal)} unsafe entry(ies): {', '.join(skipped_traversal[:3])}", file=sys.stderr, flush=True)
            with open(os.path.join(tmp_dir, "config.json"), "w") as f:
                json.dump(cfg, f, indent=4, sort_keys=False)
                f.write("\n")
            if os.path.exists(install_dir):
                # Never silently destroy the existing pack: move it aside
                # (mirrors uninstall .bak semantics) instead of rmtree.
                import time
                backup = install_dir + f".bak.{int(time.time())}"
                os.rename(install_dir, backup)
                print(f"WARNING: existing pack moved aside to {backup}", file=sys.stderr, flush=True)
            os.rename(tmp_dir, install_dir)
        except Exception:
            if os.path.exists(tmp_dir):
                shutil.rmtree(tmp_dir, ignore_errors=True)
            raise
        return soundpack_id, None


def cli_main():
    if len(sys.argv) < 2:
        print("Usage: sora-pack-import.py [--result-file PATH] <path-to-zip>")
        sys.exit(1)
    path = sys.argv[1]
    if not os.path.isfile(path):
        emit(f"ERROR:File not found: {path}")
        sys.exit(1)
    try:
        soundpack_id, err = import_zip(path)
    except Exception as e:
        emit(f"ERROR:Import failed: {_short(e)}")
        sys.exit(1)
    if err:
        emit(f"ERROR:{err}")
        sys.exit(1)
    emit(f"OK:{soundpack_id}")
    sys.exit(0)


def gui_main():
    if Gtk is None:
        emit("ERROR:GTK 4 missing — file dialog can't open")
        sys.exit(1)
    # Portal-native FileDialog — the OS default file manager dialog, so it
    # always matches whatever the user has set as default. Parent-less, as
    # originally: it now runs detached (reload-proof), so a plugin reload
    # can no longer SIGTERM it mid-dialog.
    dialog = Gtk.FileDialog(title="Import Soundpack")
    filter_zip = Gtk.FileFilter()
    filter_zip.set_name("Soundpack ZIP")
    filter_zip.add_pattern("*.zip")
    filter_all = Gtk.FileFilter()
    filter_all.set_name("All Files")
    filter_all.add_pattern("*")
    filters = Gio.ListStore.new(Gtk.FileFilter)
    filters.append(filter_zip)
    filters.append(filter_all)
    dialog.set_filters(filters)
    dialog.set_default_filter(filter_zip)

    loop = GLib.MainLoop()

    def fail_and_quit(msg):
        emit(f"ERROR:{msg}")
        loop.quit()

    def succeed_and_quit(msg):
        emit(f"OK:{msg}")
        loop.quit()

    def on_done(source, result, user_data=None):
        try:
            file = dialog.open_finish(result)
        except Exception as e:
            err = str(e)
            if "Dismissed" in err or "cancel" in err.lower():
                fail_and_quit("Cancelled")
            else:
                fail_and_quit(f"Dialog error: {e}")
            return
        if file is None:
            fail_and_quit("No file selected")
            return
        path = file.get_path()
        try:
            sid, err = import_zip(path)
        except Exception as e:
            fail_and_quit(f"Import failed: {_short(e)}")
            return
        if err:
            fail_and_quit(err)
        else:
            succeed_and_quit(sid)

    def launch_dialog():
        dialog.open(None, None, on_done, None)

    GLib.idle_add(launch_dialog)
    loop.run()


if __name__ == "__main__":
    take_result_file_argv()
    if len(sys.argv) > 1:
        cli_main()
    else:
        gui_main()
