Production plan — full plugin, phased
Phase 0 — Unblock installs (your auth, 5 min)
git push origin main (ships gate fix + sync + toast, all committed locally). Verify fresh install prints Installed prebuilt. Nothing else can be validated by users until this lands.
Phase 1 — Crash / data-loss / security (daemon + my stamp bug)
 1. STAMP sanitize (sora-build.sh): reject old_id containing .., /, null, empty, or leading - before any rm -rf; add -- to the rm. (My bug — first.)
 2. PATH hijack (auto_start.rs:12,21): absolute /usr/bin/timeout, /bin/systemctl (fallback: skip with logged error).
 3. V1 converter path sanitize (old_pack_fixer.rs:463,557): reject ..///\0 in defines filenames before join + write.
 4. Symlink containment everywhere delete_pack has it (pack_loader.rs:36-56, player.rs:628-649, commands.rs:300-329): canonicalize + starts_with check, fail closed.
 5. Atomic-write hardening (json_files.rs:31-40, converter tmps): O_EXCL create + 0600, or unique mkstemp-style names instead of pid-predictable.
 6. Resampler failure must error, not lie (sound_quality.rs:44-48): return Err, never tag wrong-rate samples as correct.
 7. No exit(1) on audio thread (player.rs:182): report audio_error via status slot, keep daemon alive for input/ctl.
 8. malloc_trim gating (pack_loader.rs:360, player.rs:571): #[cfg(target_env="gnu")]; move the engine-thread one off the audio path.
 9. Backup-restore silence (pack_loader.rs:226): surface .v1.backup copy failure instead of let _ =.
10. Corrupt-header defaults (sound_reader.rs, old_pack_fixer.rs:20): log loudly when 44.1kHz/stereo/100ms fallbacks engage; never bake silent guesses into rewritten configs.
11. Multi-pack partial failure (pack_loader.rs:289-315): collect failed keys into pack_error detail instead of log-only continue.
12. HOME-unset CWD fallback (folders.rs:9-10, main.rs:61-62, commands.rs): refuse to start or use runtime dir instead of ".".
Phase 2 — Install / update / uninstall scripts
13. sora-install:26: surface copy errors (remove 2>/dev/null || true, fail loudly); keep cp -rn semantics.
14. sora-uninstall.sh: ${HOME:?...} guard, quote $HOME, report real mv outcome (no unconditional "moved to .bak").
Plan·Muse Spark 1.3 FreeOpenCode Zen
15. sora-keyboard-access.sh: set -euo pipefail, distinguish no-privilege-tool from denied-approval (kills the infinite Retry loop).
/home/sanman/Projects
16. sorakey-detached: arg validation + usage error, -- on rm, log rotation (or delete log), propagate picker start failure.
17. sora-build.sh: trap-cleanup mktemp, quote SOURCE_DATE_EPOCH, surface curl/attestation failure reasons (archivo failing which: DNS vs 404 vs auth), corrupt-manifest diagnostic instead of silent 0.0.0.
18. dev-sync.sh: propagate restart/service failure exit codes.
19. Import/export shared emit() dedup into _v1_shared.py; fix import-side divergences (dead it, missing mkdir -p keyboard/, silent traversal skips, collision warning, size-check-before-read, no destructive overwrite without backup, export O_EXCL/prompt).
Phase 3 — QML lifecycle + dead code
20. CRITICAL SoraService.qml:29: Qt.fileExists is not a function → stoppedFlag always false → daemon auto-starts after user Stop. Replace with a polled Process/FileIO check.
21. CRITICAL SoraWidget.qml:793: stopFlagProc needs onExited — start/stop/restart must wait for flag success.
22. Exit-code checks on ctlProc, svcProc, devicesProc, pickRead, resumePoll, sectionRead/Write, logoRead/Write, roundedRead/Write (failures currently render as success/garbage).
23. Fire-and-forget execDetached (fixInTerminal, moveToSection): surface failures via errorToast.
24. Delete dead props/APIs (inputKeyboards, lastKeyAgeS, audioOk, toggle(), hovered, unset hasCursor/label/password branches) or wire them — no orphans.
25. Deduplicate: plugin ID ×10 → single source (service.pluginId), .slice(0,500) ×9 → helper, debounce/radius constants → shared.
26. Text consistency pass (pack vs Sound, sounds vs permission, ellipsis/case style) + fix 2 stale SearchableDropdown comments + SoraKeyStore.js example.
27. SoraDropdown.qml:161: empty-options state (mirror PackPicker's emptyText).
Phase 4 — CI / release hardening
28. New ci.yml: test + clippy + fmt on every push/PR to main (the missing gate that let E0583 reach a tag).
29. release.yml: delete duplicated deps step; remove x86_64-only gate on version check; pin all actions by SHA; gate rust-toolchain.toml agreement; fix grep -m1 '^version' fragility; attest SHA256SUMS too.
30. Unify the three version parsers (release.yml:31, sora-build.sh:62, version_check.rs:19) into one strict form.
31. Narrow-gate follow-ups: add Cargo.lock to gate paths; manifest-version-only comparison (close the two holes I disclosed).
Phase 5 — Docs (make them true again)
32. Correct soundpackRules.md: options allowed when recommended_volume ≠ 1.0; fix codec list (m4a/aac allowed per Cargo); soften key-order mandate; fix per-key audio_file claim vs epomaker reality; fix bare-command live-test line.
33. docs/dev/plan.md: mark frozen audit as historical (its release.yml claims and all Panel.qml line refs are false now) or refresh Part 1/2 + contradicted items.
34. docs/dev/relse.md + note.md: resolve --tags contradiction; clarify Sora*.qml scope; pin toolchain in one place.
35. README.md: complete Files table, align Remove vs --purge data-loss expectations; CLEANUP.md already accurate.
36. rename.md: refresh stale path tables or archive (rename done).
Verification per phase
- Phases 1–2: cargo test + clippy -D warnings + fmt --check green after every edit group; script sims (fake $HOME) for sync/uninstall/access paths; bash -n + py_compile.
- Phase 3: qmllint if present, else panel walk-through on dev-sync (import/export/update/delete/stop/start/toasts).
- Phase 4: green CI run on a branch before merging.
- Phase 5: rg sweeps for every renamed/deleted name come back empty outside docs/dev/ history.
Suggested execution order: 0 → 1 (items 1–6 first, then rest) → 4 (28 first, so later phases are gated) → 2 → 3 → 5 → 6 (the import-security half of Phase 2, biggest blast radius, last). Say go phase 1 (or go all, or any subset) and I'll execute in build mode — no commits unless you say so.