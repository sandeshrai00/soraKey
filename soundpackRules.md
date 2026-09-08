# Soundpack rules — how to add a pack

Follow this exactly when adding a pack to `daemon/soundpacks/keyboard/`.
The CI test `every_bundled_soundpack_validates` (`daemon/src/utils/pack_checker.rs`)
validates EVERY `config.json` recursively — one bad file fails the whole suite.

## 1. Folder layout (flat, no nesting)

```
daemon/soundpacks/keyboard/<pack-id>/
├── config.json
├── <PACK>_KEY.<ext>        single-method: exactly one audio file
└── <pack-id>.jpg           optional icon (may omit; none of the
                           bundled packs ships one)
```

- `<pack-id>`: lowercase-hyphen only (`gravastar-v60-pro`). It becomes the
  runtime id `keyboard/<pack-id>` — the daemon derives identity from the
  directory name (`pack_info.rs`), not from the config's `id` field.
- Multi-method: all per-key files live in the SAME folder
  (`SORA_KEY1.wav` … `SORA_KEY8.wav`, `SORA_BACKSPACE.wav`).
  No subdirectories, no spaces in any path — ever.
- Never put a second `config.json` in a subfolder. The scanner ignores it,
  but the CI validator does not (a legacy V1 file will fail the build).

## 2. Audio files

- Names: UPPERCASE `<PACK>_KEY.<ext>`, extension stays lowercase
  (`AULA_KEY.wav`, `GRAVASTAR_KEY.mp3`, `SORA_KEY1.wav`).
- Formats that decode: `wav`, `mp3`, `ogg`
  (symphonia features in `daemon/Cargo.toml`). Nothing else.
- Audio bytes are never edited — renames are filename-only (verify with md5).

## 3. config.json — exact shape

Top-level keys in THIS order, no more, no less:

```json
{
    "id": "<pack-id>",
    "name": "<Display Name>",
    "author": "sorakey",
    "config_version": "2",
    "created_at": "<fresh UTC ISO-8601>",
    "definition_method": "single",
    "audio_file": "<PACK>_KEY.<ext>",
    "definitions": {
        "Space": { "timing": [[24626.0, 24697.5], [24697.5, 24769.0]] }
    }
}
```

- `id`: bare `<pack-id>`, NO `keyboard/` prefix (runtime adds it).
  Must equal the folder name.
- `name`: Title Case display name, no attribution suffixes
  (`"Gravastar V60 Pro"`, not `"Gravastar V2 Pro - By Saint"`).
- `author`: always `"sorakey"` for bundled packs.
- `config_version`: always `"2"` (string).
- `created_at`: fresh UTC timestamp at pack-creation time, e.g.
  `2026-09-08T06:23:05.817804+00:00`. Never copy another pack's date.
- `definition_method`: `"single"` (one shared file) or `"multi"`
  (per-key files).
- Single-method: top-level `audio_file` is REQUIRED (must exist on disk).
- Multi-method: NO top-level `audio_file`. Each key maps to its own file:
  `"Escape": {"timing": [[0.0, 183.4]], "audio_file": "SORA_KEY1.wav"}`.
- `definitions`: every entry is `{"timing": [[start, end], ...]}`.
  Timings are finite numbers with `start < end`. Copy them verbatim from
  the source pack — never hand-edit a number.
- **Full coverage is mandatory.** Every pack must define all 129 keys of
  the reference set (`epomaker-rt85` is the reference). A missing key is
  silent at runtime — the engine skips it with no error
  (`player.rs:handle_key_event`). Fill gaps by deep-copying the timing
  window of the closest donor (same digit for numpad digits, mirror
  modifier, same key family, neutral click for media keys) — never by
  inventing timings.

## 4. FORBIDDEN keys (do not add)

`options`, `tags`, `description`, `soundpack_type`, `defs`, `defines`,
`icon` (unless a real jpg ships with the pack), `version`, `sound`.

- `options`/`tags`/`description`: stripped from all bundled packs.
  (A `recommended_volume` of 1.0 is the default anyway; `tags:
  ["pre-installed"]` is only for release-time batch edits, never by hand.)
- `soundpack_type`: dead — nothing in `daemon/src` reads it.
- `defs`/`defines`: legacy V1 spellings. The validator accepts `defs` as
  an alias, but shipping both doubles file size for zero benefit.
- `icon` pointing at a file that doesn't exist: the picker falls back,
  but don't ship the lie — either add the jpg or omit the key.

## 5. Checklist before commit

1. `python3 -m json.tool <pack>/config.json` parses.
2. Every `audio_file` in the config exists on disk (single: 1 ref,
   multi: check all — Epomaker has 129).
3. No `..`, no spaces, no absolute paths in any `audio_file` value
   (the loader skips such entries silently → dead keys).
4. Timings diffed against the source pack: identical, key-for-key.
5. `cargo test` passes (runs `every_bundled_soundpack_validates`).
6. Folder holds exactly `config.json + audio (+ optional jpg)`.

## 6. Making it the default / shipping it

- Fresh-install default is ONE line:
  `daemon/src/state/settings.rs` → `Default for AppConfig` →
  `keyboard_soundpack: "keyboard/<pack-id>"`.
- Any `daemon/` edit (including that line) requires a version bump +
  tag + release, or users build from source
  (see `docs/dev/relse.md`). Pack-files-only changes need NO release.
- Existing users keep their saved `keyboard_soundpack` — a new default
  affects fresh installs only. To migrate existing users, add a
  `migrate(old, new)` entry in `settings.rs` next to the others.
- Live-test: `sora-pack-import.py` the pack (or copy to
  `~/.local/share/sorakey/soundpacks/keyboard/`), `./admin-scripts/dev-sync.sh`,
  switch packs in the panel, type keys, listen.
