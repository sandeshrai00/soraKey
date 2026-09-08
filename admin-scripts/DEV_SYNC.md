# dev-sync — local plugin testing without git push

Test edits locally before committing. No `git push` needed.

## What it does

`dev-sync.sh` copies `sorakey/` (dev repo) → `~/.config/omarchy/plugins/io.github.sandeshrai00.sorakey/` (installed plugin), then validates and restarts the shell.

```
dev repo  (edits here)
   |
   | rsync -a --delete
   v
installed plugin dir  →  omarchy plugin validate  →  omarchy restart shell  →  journalctl / systemctl check
```

- Mirrors all files, deletes stale ones (so removed files disappear like a fresh install).
- Syncs `.git` so `omarchy plugin update` still works (but will be dirty until you commit).
- Uses `rsync` if available, falls back to `cp -a`.

## Usage

```bash
# from repo root
./admin-scripts/dev-sync.sh
# or
bash admin-scripts/dev-sync.sh

# skip shell restart (just sync + validate)
./admin-scripts/dev-sync.sh --no-restart

# skip validation
./admin-scripts/dev-sync.sh --no-validate
```

Check result:

```bash
journalctl --user -n 100 | grep -i sorakey
systemctl --user is-active sorakey
~/.local/bin/sorakey ctl '{"cmd":"status"}'  # daemon alive
```

## Fresh-install simulation without push

```bash
omarchy plugin remove io.github.sandeshrai00.sorakey --yes  # wipes installed dir
./admin-scripts/dev-sync.sh                                  # recreates it from dev repo
```

`omarchy plugin add` requires a git URL, so local `cp/rsync` is the official dev loop.

## QML vs daemon

| Edit | Needs new release? | How dev-sync tests it |
|------|-------------------|----------------------|
| `SoraWidget.qml` / `SoraService.qml` / `scripts/` | No | dev-sync + shell restart is enough |
| `daemon/` (Rust) | Yes — every main push publishes the matching CI prebuilt | dev-sync + shell restart triggers `sora-build.sh`: hash match → prebuilt installs; no match yet (CI still building ~10 min) → loud refusal, current binary kept. Local uncommitted edits can never match: devs build with `SORAKEY_ALLOW_SOURCE=1 ./scripts/sora-build.sh` |

## Notes

- After dev-sync the installed dir is dirty (`git status` shows your uncommitted changes). `omarchy plugin update` will fail with `merge --ff-only` until you `git commit`/`git -C ~/.config/omarchy/plugins/... reset --hard origin/main`.
- Prebuilt check (`release_matches_source`) only trusts a release when `manifest.json:version` has a matching `vX.Y.Z` tag and `daemon/` is clean vs that tag. Untagged local daemon edits always fall to source build - expected.
