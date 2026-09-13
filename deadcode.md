# Sorakey — dead-code cleanup plan (YAGNI audit)

Audit of every code file in the plugin (Rust daemon, 7 QML/JS files, scripts,
admin-scripts, udev, assets, .github, manifest). Method: ponytail/YAGNI —
delete what nothing ships, calls, or reads. Every "certain" finding below was
verified by direct grep/read, not just inferred.

Legend: **DEAD** = zero consumers, safe to delete · **FLAGGED** = needs a
decision (defaults marked) · **KEEP** = looked dead, verified alive.

Line numbers refer to the tree at audit time (post v0.1.9 consent removal).

---

## Phase 1 — Rust daemon (≈1,050 lines)

### 1a. `SoundpackCache` — write-only fossil (≈700 lines, biggest win)

Full scan → validate → serialize of every pack at startup and on every
load/delete, feeding `~/.local/share/sorakey/data/soundpack_cache.json`.
**Nothing reads that file.** The panel's pack list comes from the `packs`
ctl cmd's live directory scan (commands.rs:577), not the cache. Sole cache
reader is `SoundpackCache::load()` at commands.rs:662 — the dead `diag`
cmd (see 1c).

| Delete | Where |
|---|---|
| `CACHE_LOCK` + `cache_lock()` | state/packs.rs:8-22 |
| `SoundpackOptions` + `default_recommended_volume` + `default_random_pitch` + `impl Default` | state/packs.rs:28-51 |
| `SoundpackMetadata` struct | state/packs.rs:91-109 |
| `SoundpackCache` struct + entire impl (`load`, `load_locked`, `new`, `save_locked`, `add_soundpack`, `refresh_from_directory`, `update_count`, `scan_soundpack_type`, `insert_error_metadata`, `cache_file`) | state/packs.rs:111-300 |
| `utils/pack_info.rs` — **entire file** (181 lines; its only fn `load_soundpack_metadata` feeds the cache) | utils/pack_info.rs |
| `create_soundpack_metadata` | libs/pack_loader.rs:151-199 |
| `update_soundpack_cache` | libs/pack_loader.rs:493-506 |
| `capture_soundpack_loading_error` | libs/pack_loader.rs:508-566 |
| `use ...{SoundpackCache, SoundpackMetadata}` / `SoundPack`-metadata imports | libs/pack_loader.rs:3 |
| cache write calls (`update_soundpack_cache` / `capture_soundpack_loading_error`) | libs/player.rs:564, 567 |
| `update_cache_on_error` field on `AudioCommand::LoadKeyboardPack` — exists only to steer the dead cache (not wire-serialized; crossbeam in-process) | libs/player.rs:39, 557, 566, 591, 616, 625 + call sites commands.rs:380, 530, 538 |
| cache block in `delete_pack` (lock + remove + save, with its comment) | commands.rs:488-492 |
| `soundpack_cache_json()` | state/folders.rs:30 |

**`SoundPack` struct slimming** (state/packs.rs:62-89): keep only fields with
a real reader — `definition_method`, `definitions`, `audio_file`, `name`
(log at pack_loader.rs:360). Delete: `id`, `description`, `author`, `version`,
`config_version`, `icon`, `license`, `tags`, `created_at`, `options`,
`config_version_num`. Serde ignores unknown JSON fields in pack configs — no
break.

### 1b. `sorakey key` CLI chain (≈130 lines) — zero invokers

The binary's `key` subcommand and its whole path exist "for notifiers" —
no notifier exists in the repo, nothing documents or calls it. The engine's
real keyboard path uses raw `"KeyA"` strings off `keyboard_rx`
(player.rs:827-833), never this variant.

| Delete | Where |
|---|---|
| `key` subcommand block | main.rs:18-23 |
| `key_client` | commands.rs:765-779 |
| `"key"` match arm | commands.rs:141 |
| `key_event` | commands.rs:204-221 |
| `is_known_key_code` | commands.rs:223-307 |
| `AudioCommand::Key` variant + its match arm | libs/player.rs:33-36, 586-588 |

Bonus: after this, the enum doc comment ("Keyboard key events are NOT a
variant here") becomes true.

### 1c. Dead ctl commands (≈50 lines)

| cmd | Delete | Evidence |
|---|---|---|
| `toggle_mute` | arm commands.rs:142 + handler :309-318 | zero senders (QML/scripts/CI). Mute works via `mute` cmd (SoraWidget.qml:350) and Ctrl+Alt+M → hotkey listener → `SetSoundEnabled`, independent of this arm |
| `diag` | arm :139 + `diag()` :658-680 + `proc_kb` :645-656 | only referenced in README prose |

### 1d. `auto_start` field (≈60 lines) — written by a boot-time probe, read by nothing

Not in the `status` response, not in QML, not in scripts. The sync block
spawns `systemctl --user is-enabled sorakey` **at every daemon start** to
maintain a field no consumer reads.

| Delete | Where |
|---|---|
| `auto_start` field + default | state/settings.rs:25, 308 |
| `data_equals` line | state/settings.rs:159 |
| sync block (the per-boot `systemctl` probe) | state/settings.rs:213-233 |
| `utils/auto_start.rs` — **entire file** | — |
| 5 test references to the field | state/settings_saver.rs:104-106, 118, 138-150, 286, 316-317 |

Old `config.json` files that still contain `auto_start` keep loading (lenient
parse) — no migration.

### 1e. Small confirmed dead items (≈30 lines)

| Item | Where | Evidence |
|---|---|---|
| `GENERATION` AtomicU64 + `fetch_add` | state/settings_saver.rs:11, 52-53 | write-only counter; comment references a `generation()` fn that doesn't exist |
| `commit` field + `option_env!("GIT_HASH")` | state/settings.rs:14, 302 | no build.rs, no CI env var — always `None`, never read |
| `impl Default for DeviceManager` | libs/speakers.rs:181-185 | zero callers (all sites use `::new()`) |
| `SoundpackValidationResult.detected_version` + 7 write sites | utils/pack_checker.rs:36, 53, 67, 96, 109, 143, 263, 272, 281 | never read |
| `SoundpackValidationStatus::InvalidVersion` variant | utils/pack_checker.rs:7 | never constructed; its only match arm dies with pack_info.rs (1a) |

### 1f. Dead tests (≈180 lines) — tautologies and tests of a test-local reimplementation

| Delete | Where | Why |
|---|---|---|
| `global_mute_silences_keyboard` | libs/player.rs:953-959 | `let sound_enabled = false; assert!(!sound_enabled)` — calls no production code |
| `set_sound_enabled_command_moves_engine_state` | libs/player.rs:961-973 | pattern-matches a locally-constructed command and asserts the local var |
| test-local `Authority` struct | state/settings_saver.rs:64-89 | reimplements `apply` inside the test module; tests below drive it, not the real code |
| `a_slow_writer_finishing_late_does_not_revert_a_concurrent_change` | settings_saver.rs:125-153 | duplicate of :93, same 3-field scenario |
| `a_long_lived_writer_does_not_restore_launch_state` | settings_saver.rs:157+ | drives `Authority` |
| `a_deferred_write_landing_late_touches_only_its_own_field` | settings_saver.rs:184+ | drives `Authority` |
| `re_asserting_the_current_value_writes_nothing` | settings_saver.rs:219+ | drives `Authority` |
| `a_metadata_only_mutation_does_not_desync_memory_from_disk` | settings_saver.rs:377+ | drives `Authority` |
| `holding_a_struct_and_writing_it_back_is_what_reverts_a_concurrent_change` | state/settings.rs:318-339 | pure clone/field assertions, calls no production code |

Real `apply` concurrency stays covered by the two thread tests that lock the
real authority: settings_saver.rs:254, :323.

**Phase 1 gate:** `cargo test --offline` (all remaining green),
`cargo clippy --offline --all-targets` (0 warnings), `cargo fmt --check`,
release build.

---

## Phase 2 — QML/JS (≈150 lines)

| Delete | Where | Evidence |
|---|---|---|
| `clearUpdateTimer` Timer | SoraWidget.qml:605-609 | never started anywhere (grep = 1 match, the `id:` line) |
| panel-level `applyStatus()` wrapper fn | SoraWidget.qml:661-666 | zero callers — statusProc.onExited inlines the same logic |
| `packOptions(ids)` helper | SoraKeyStore.js:18-27 | superseded by `packOptionsDetailed`; zero `Model.packOptions` calls |
| `toggle()` | SoraPackPicker.qml:32, SoraDropdown.qml:58 | zero callers |
| `hovered(bool)` signal + its emit | SoraDropdown.qml:61, 110 | emitted but never handled (no `onHovered` anywhere) |
| 10 unused ids (remove the `id:` token only) | SoraWidget.qml: `checkingSpinner` :1442, `checkingText` :1455, `kbSlider` :1661, `kbPack` :1708, `openFolderButton` :1754, `transportStop` :1771, `transportRestart` :1781, `transportShuffle` :1792 · SoraPackPicker.qml: `listContainer` :275, `confirmSep` :429 | each grep = exactly 1 match |

**22 fork-knob properties never overridden by the sole consumer** (inline
defaults at the use sites, delete the properties):

- SoraPackPicker.qml: `label` :11, `showLabel` :25, `hasCursor` :26,
  `emptyText` :15, `background` :17, `accent` :19, `fontFamily` :21,
  `popupRowHeight` :23, `popupMinHeight` :24
- SoraDropdown.qml: `label` :30, `showLabel` :43, `hasCursor` :49,
  `emptyText` :33, `background` :36, `accent` :38, `fontFamily` :40,
  `popupRowHeight` :42
- SoraTextField.qml: `selectionTint` :31, `password` :32,
  `horizontalPadding` :33, `verticalPadding` :34, `hasCursor` :41

Consequences: the never-visible label `Text` items get deleted with
`label`/`showLabel` (they can never pass their `visible:` check); `_hot`
simplifies to hover-only (`hasCursor` never set true); `emptyText` default
"No matches" inlined at its single use; `password`/`echoMode` always Normal.

**KEEP deliberately:**

- `SoraService.qml:18 pluginId` — zero in-repo reads, but the omarchy shell
  (outside this repo) instantiates this file and could read it; 1 line, not
  worth a runtime gamble. Revisit only if confirmed unused by the shell.
- Picker/Dropdown fork duplication (`optionValue`/`optionLabel`/
  `currentLabel`, border blocks) — intentional per file headers ("keep the
  two separate").

**Phase 2 gate:** `omarchy restart shell` → panel opens; picker select +
search + delete confirm + fallback label (the v-next fix) all still work;
dropdowns + textfield render identically.

---

## Phase 3 — scripts (≈30 lines)

| Delete | Where | Evidence |
|---|---|---|
| non-`--purge` branch (the `.bak` keep-data path) + `PURGE` var | scripts/sora-uninstall.sh:6-7, 31-43 | sole caller (SoraWidget.qml:641) always passes `--purge`; orphan self-clean (daemon) wipes directly and never calls this script. Script becomes purge-only, still accepts the flag |
| result-file writes (3× `[[ -n $result ]] && printf ... >> "$result"`) + `result="${1:-}"` | scripts/sora-update.sh:16, 41, 46, 51 | written, read by nothing — real feedback is the `notify()` calls. Keep `$1` positionally: sorakey-detached's jail requires the result-file arg (sorakey-detached:17-24) |
| stale docstring "import + admin converter" | scripts/_v1_shared.py:1 | no admin converter exists → "import + export" |

**KEEP deliberately:**

- Test-seam env vars `SORAKEY_UPDATE_NO_RESTART` (sora-update.sh:25),
  `SORAKEY_RELEASE_BASE` (sora-build.sh:26), `SORAKEY_ALLOW_SOURCE`
  (sora-build.sh:275) — the only way to exercise these scripts locally.
- `sora-keyboard-revoke.sh` — never auto-invoked by design (manual `sudo`
  tool, staged to `~/.local/lib/sorakey/` by sora-build.sh).

**Phase 3 gate:** `bash -n` every script; dry uninstall sim with a fake HOME
(purge path wipes, `.bak` logic gone, no errors).

---

## Phase 4 — repo hygiene (≈5 lines)

| Delete | Where | Evidence |
|---|---|---|
| `build/` entry | .gitignore:2 | no such path exists or is ever created in-repo (CI builds into `daemon/target/`) |
| `dist/` entry | .gitignore:3 | only ever created inside the CI runner, never in a checkout |
| `*.log` entry | .gitignore:4 | daemon writes no `.log` files (0 hits in src); detached sidecars live in `~/.cache/sorakey`, outside the repo |
| `scripts/__pycache__/` (3 untracked .pyc) | on disk only | local bytecode, gitignored, zero repo impact |

---

## Flagged — decisions pending (defaults in parens)

1. **`admin-scripts/`** (dev-sync.sh + DEV_SYNC.md + IOHOOK_KEYCODES.md +
   README.md) — zero references from the shipped product (QML, CI, scripts,
   manifest, daemon). Hand-run dev tool (rsync checkout → installed plugin).
   Default: **keep** (it is this repo's dev loop).
2. **`ssss.md`** (repo root, tracked, scratch file) — Default: **delete**.
3. **Python `cli_main()`** — manual-terminal import/export, no in-repo
   caller (panel does both via file picker):
   sora-pack-import.py:484-501 + dispatch :568-569,
   sora-export-logs.py:135-163 + dispatch :168-169.
   Default: **delete** (revivable from git history).
4. **`AppConfig.version` + `AppConfig.last_updated`** (state/settings.rs:12-13)
   — written to config.json every save, read by no code. Pure persisted
   metadata. Default: **delete**.
5. **`sora-build.sh:140`** `*) arch="x86_64"` silent fallback — on an
   unsupported arch it downloads an unrunnable binary with no warning. Not
   dead code; optional hardening to fail loudly. Default: **leave** (separate
   change if wanted).

## Verified NOT dead (checked, left alone)

- All 12 Cargo deps (chrono, crossbeam-channel, evdev, directories, hound,
  libc, rodio, cpal, rubato, serde, serde_json, symphonia) — each has a
  live use. Symphonia `isomp4`/`aac` features are the only speculative ones
  (no m4a pack in repo; user imports decide the real format set) — keep.
- `utils/keys.rs KEY_MAP` — looks like it belongs to the dead `key` chain
  but is used by the live V1→V2 converter (old_pack_fixer.rs:769).
- Every `folders::*`, `status::*`, `settings::parse_lenient/save/data_equals/
  effective_volume`, `json_files::*`, `files::*` (except the flagged
  `pub`-on-`unique_sibling_temp`), `speakers::*` fns, all `old_pack_fixer`
  fns, `pack_checker::validate_soundpack_config` + `can_be_converted`,
  `logs::*` (via live `export_logs`), `orphan::*`.
- All 4 assets (icon-bar-dark/light.svg, icon-hero-dark/light.svg).
- `udev/70-sora-keyboard.rules` — the only rule file, installed by
  sora-keyboard-access.sh:23,56. The legacy `70-sorakey-keyboard.rules` name
  exists only as `/etc` cleanup constants (correct: they target system
  leftovers).
- Both CI workflows (ci.yml push/PR on main; release.yml on `v*` tags — tags
  exist and versions agree at 0.1.9/1.87, enforced release.yml:39-44).
- All manifest.json keys (contract fields; `id`/`version`/`entryPoints` have
  in-repo consumers, the rest are omarchy contract).
- rust-toolchain.toml (1.87, matches both workflows).
- All 11 scripts in scripts/ are invoked (map in audit; `_v1_shared.py` also
  runs in CI via the old_pack_fixer parity test).
- `SoraAppStore.qml`: every store field + derived prop is read by a view.
- QML fork-duplication (Phase 2 KEEP list).

## Execution order

1. Phase 1 (Rust) → Phase 1 gate (test/clippy/fmt/build)
2. Phase 2 (QML/JS) → Phase 2 gate (shell restart, panel pass)
3. Phase 3 (scripts) → Phase 3 gate (bash -n + uninstall sim)
4. Phase 4 (hygiene)
5. Resolve flagged items 1-5
6. Bump 0.1.9 → **0.1.10** (manifest.json + daemon/Cargo.toml +
   daemon/Cargo.lock — daemon `.rs` changed, release rule requires tag)
7. User: commit → `git tag v0.1.10` → push → CI →
   `omarchy plugin update io.github.sandeshrai00.sorakey --yes && omarchy restart shell`
8. Acceptance: panel works end-to-end, delete-current-pack shows the fallback
   name (previous fix still intact), uninstall purge path verified,
   `sorakey key`/`toggle_mute`/`diag` confirmed gone from the binary
   (`sorakey key X` → "already running"-style fallback is fine — the arg
   parse is just gone).

## Expected impact

≈1,250-1,300 lines deleted: ~700 cache subsystem, ~130 key chain, ~50 ctl
cmds, ~60 auto_start, ~30 misc Rust, ~180 dead tests, ~150 QML/JS, ~30
scripts, ~5 hygiene. No behavior change for any live path; startup gets
faster (no full pack metadata scan at boot, no per-boot `systemctl` probe).
