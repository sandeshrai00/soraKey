# Sorakey release guide (copy-paste ready)

Repo: github.com/sandeshrai00/soraKey
Binaries built by CI: `sorakey-x86_64` + `sorakey-aarch64`
(Rust 1.87, pinned in rust-toolchain.toml and mirrored in
.github/workflows — CI's identity check fails the run if they disagree)

---

## 0. The one rule (read this first)

A release exists for ONE thing: shipping the compiled **daemon binary**.
The installer (`scripts/sora-build.sh`) only downloads the GitHub
prebuilt when **the tagged commit's compiled inputs (`.rs`, `Cargo.toml`,
`Cargo.lock`, `rust-toolchain.toml`) match your current source**.
Soundpacks are data, not compiled in — pack-only changes never need a
release. Otherwise every user compiles from source (slow, needs Rust).

| What you changed | New release? |
|---|---|
| any `.rs`, `Cargo.toml`, `Cargo.lock` under `daemon/` | **YES — must release** |
| `rust-toolchain.toml` | **YES — must release** |
| only `daemon/soundpacks/**` (pack data) | **NO** — data ships from the repo; the installer syncs it on next shell start |
| only QML/JS (`SoraWidget.qml`, `SoraService.qml`, `SoraKeyStore.js`, `SoraPackPicker.qml`, `SoraDropdown.qml`, `SoraTextField.qml`), `scripts/*`, README, docs | **NO** — plugin files ship from the repo; users get them with `omarchy plugin update` |

Four things must ALWAYS agree with each other: the git tag,
`manifest.json:version`, `daemon/Cargo.toml:version`, **and
`daemon/Cargo.lock`** (its `sorakey` entry must say the new version too).
CI runs `cargo --locked` and fails the run if the lockfile disagrees.

---

## 1. Which situation are you in? (pick ONE, follow only that block)

### Situation A — Normal new version (e.g. 0.1.1 → 0.1.2)

You changed `daemon/` and the last release already exists on GitHub.

```bash
# 1. Bump BOTH version files to the new number (they must match):
#    manifest.json  ->  "version": "0.1.2",
#    daemon/Cargo.toml  ->  version = "0.1.2"

# 2. Sync the lockfile — CI runs `cargo --locked` and fails in ~30s
#    if Cargo.lock still says the old version. This step is easy to
#    forget; never skip it:
cargo update --manifest-path daemon/Cargo.toml --offline 2>/dev/null \
  || cargo update --manifest-path daemon/Cargo.toml
grep -m1 -A1 'name = "sorakey"' daemon/Cargo.lock
# must print version = "0.1.2" — if it still says 0.1.1, STOP and rerun above

# 3. Commit ONLY the bump (all THREE files):
git add manifest.json daemon/Cargo.toml daemon/Cargo.lock
git commit -m "bump 0.1.2"

# 4. Push code, then tag, then push THAT ONE tag (not --tags):
git push origin main
git tag v0.1.2
git push origin v0.1.2
```

Tag name is exactly `v` + version: `v0.1.2`.
Not `0.1.2`. Not `v1.2`. The tag, manifest, and Cargo.toml must all
say the same number or CI refuses to build.

### Situation B — Same version again after a FULL delete

You deleted the release AND its tag on GitHub (releases page is empty),
and you want the same number back (e.g. `v0.1.1` again).
This is allowed ONLY because the tag is gone. Otherwise: always bump
(see Situation A).

```bash
# 1. No version bump needed — but VERIFY both files agree:
python3 -c 'import json; print(json.load(open("manifest.json"))["version"])'
grep -m1 '^version' daemon/Cargo.toml
# both lines must print the same number. If not, fix them first.

# 2. Delete any stale LOCAL tags with this number (or older junk):
git tag -d v0.1.1 v0.1.2 v0.1.3 2>/dev/null; true
# (adjust the list to whatever `git tag` shows locally)

# 3. Push code, then tag, then push THAT ONE tag:
git push origin main
git tag v0.1.1
git push origin v0.1.1
```

Why step 2 matters: `git push --tags` would resurrect every stale local
tag onto your clean remote and fire CI on old commits. Push ONE tag,
by name, always.

### Situation C — No tags on GitHub at all (starting from scratch)

Releases page empty, `git ls-remote --tags origin` empty. Same as
Situation B: pick your starting number (whatever `manifest.json`
already says is fine), make sure `daemon/Cargo.toml` matches it,
push main, tag, push the one tag. There is no "first release" magic —
CI treats every tag push the same.

```bash
git push origin main
git tag v0.1.1
git push origin v0.1.1
```

### Situation D — Only QML/scripts/docs changed (no release at all)

Do nothing release-related. Just push main:

```bash
git push origin main
```

Users get the files via `omarchy plugin update`. The daemon prebuilt
keeps working because the daemon source didn't move. DO NOT tag —
an identical-source tag only restarts CI for no reason.

---

## 2. What CI does after you push the tag (~5–8 min)

Open: https://github.com/sandeshrai00/soraKey/actions
Workflow: `release-sorakey`. It:

1. checks tag == `manifest.json` version == `daemon/Cargo.toml` version
   (any mismatch → run fails here, nothing ships);
2. runs `cargo test` + `clippy -D warnings` + `cargo fmt --check`
   (red suite → run fails here, nothing ships);
3. builds the daemon on x86_64 AND aarch64 runners;
4. merges both into one `SHA256SUMS`, attests both binaries;
5. creates **Release vX.Y.Z** with exactly 3 assets:
   `sorakey-x86_64`, `sorakey-aarch64`, `SHA256SUMS`.

## 3. Verify the release landed

Open: https://github.com/sandeshrai00/soraKey/releases

You must see your version with exactly **3 assets**.
If an arch is missing, that runner failed — open its job log.

Then on your machine (fresh state test):

```bash
omarchy restart shell
sleep 5
journalctl --user --since "1 min ago" | grep -i sorakey
```

Expected: `Installed verified prebuilt X.Y.Z ... (attested)` or
`... up to date`. If you see `building from source`, the tag and the
source disagree — see "Fixing mistakes" below.

---

## 4. Writing the release description

CI auto-generates notes from commit titles
(`generate_release_notes: true`) — `feat:`/`fix:` lines show up on
their own. To write your own:

1. GitHub → **Releases** → click the release;
2. pencil icon (Edit) next to the title;
3. replace the body, keep the title as `vX.Y.Z`;
4. **Update**.

Template:

```markdown
## What's new
- (one line per user-visible change)

## Install / update
- Existing users: `omarchy plugin update` (or restart the shell) — the
  verified prebuilt binary downloads automatically.
- Fresh install: `omarchy plugin add https://github.com/sandeshrai00/soraKey.git`

## Assets
- `sorakey-x86_64` — prebuilt daemon (x86_64 Linux, Rust 1.87)
- `sorakey-aarch64` — prebuilt daemon (ARM64 Linux, Rust 1.87)
- `SHA256SUMS` — checksums (verified + attested by CI)
```

To list what changed since the last release (for "What's new"):

```bash
git log --oneline v0.1.0..HEAD        # replace v0.1.0 with the PREVIOUS tag
```

(If there is no previous tag yet, plain `git log --oneline` lists
everything — the release is the whole history.)

---

## 5. Fixing mistakes (which fix for which problem)

**CI failed at "Validate release identity" (tag/version mismatch):**
your tag says one number, a version file says another. Fix the files,
commit, delete the tag locally AND remotely, re-tag, push the one tag:

```bash
git tag -d v0.1.2
git push origin :refs/tags/v0.1.2
git tag v0.1.2
git push origin v0.1.2
```

**You committed daemon changes AFTER tagging (common):**
the prebuilt is now stale — users build from source until you release
again. Don't re-tag the same number (the tag already exists → CI's
`release_matches_source` logic and GitHub both treat tags as permanent).
Bump and follow Situation A.

**CI failed in ~30s with `lock file needs to be updated but --locked was passed`:**
you bumped `manifest.json` + `daemon/Cargo.toml` but forgot
`daemon/Cargo.lock` (it still says the old version). One commit, one
force-tag — stay on the same number, no bump needed:

```bash
cargo update --manifest-path daemon/Cargo.toml --offline 2>/dev/null \
  || cargo update --manifest-path daemon/Cargo.toml
grep -m1 -A1 'name = "sorakey"' daemon/Cargo.lock
# must print the NEW version — if not, STOP, the update didn't take

git add daemon/Cargo.lock
git commit -m "fix: sync Cargo.lock to 0.1.2 (bump omitted it, broke --locked)"
git push origin main
git tag -f v0.1.2
git push -f origin tag v0.1.2  # re-runs release on the fixed commit
# watch https://github.com/sandeshrai00/soraKey/actions → sorakey-ci green
# ~30s, release-sorakey green ~5-8 min → v0.1.2 shows 4 assets
```

**CI failed at tests/clippy/fmt:**
fix the code locally until `cargo test`, `cargo clippy -- -D warnings`,
and `cargo fmt --check` are all green (run them in `daemon/`), commit,
then delete + re-tag + push (same commands as the mismatch fix above).
The tag must move to the fixed commit — CI always builds the tagged
commit, never main.

**Only the description is wrong:**
pencil icon, edit, Update. Release FILES can never be replaced — a new
binary always means a new tag (or Situation B after a full delete).

**Reusing a version number:**
normally impossible (tags are permanent). The ONLY exception is
Situation B: release + tag both fully deleted first. Otherwise bump.

---

## 6. Pre-release (unstable build, optional)

A pre-release is a normal release **marked** "pre-release". It downloads
exactly the same way; on GitHub it just sits under *Pre-releases*.
Use it for experimental daemon changes you want testable but not final.
Promoting it to final later is one checkbox — **no new tag needed**.

After CI created the release, GitHub UI: open release → pencil →
check **"This is a pre-release"** → Update. (Or
`gh release edit vX.Y.Z --prerelease`; needs `gh auth login`.)

Don't add `prerelease: true` to the workflow — that would mark EVERY
release as pre-release. CI always creates normal releases; the mark is
your manual step.

---

## 7. Deleting a release (last resort)

**Warning:** deleting removes the binary. Every user on that version
falls back to compiling from source until you ship a replacement. If
only the description is wrong — edit it, don't delete.

- **Release + tag:** GitHub UI → open release → bottom →
  "Delete release" → confirm; then delete the tag too, or it keeps
  pointing at the old commit:
  `git push origin :refs/tags/vX.Y.Z` (+ `git tag -d vX.Y.Z` locally).
- **Tag only:** `git tag -d vX.Y.Z` + `git push origin :refs/tags/vX.Y.Z`.
- After a FULL delete (release gone, tag gone), reusing the number is
  allowed → follow Situation B.

---

## 8. Quick sanity checks (paste anytime)

```bash
# versions agree? (all three must print the same number — including the lock)
python3 -c 'import json; print(json.load(open("manifest.json"))["version"])'
grep -m1 '^version' daemon/Cargo.toml
grep -m1 -A1 'name = "sorakey"' daemon/Cargo.lock

# do I need a release? (any output = yes)
# NOTE: needs at least one tag to compare against. No tags yet?
# use: git diff --name-only HEAD -- daemon rust-toolchain.toml
git diff --name-only $(git describe --tags --abbrev=0 2>/dev/null || echo HEAD)..HEAD -- daemon rust-toolchain.toml

# what will the next release contain?
git log --oneline
```
