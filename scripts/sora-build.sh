#!/bin/bash
set -euo pipefail
PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DAEMON_DIR="$PLUGIN_DIR/daemon"
MANIFEST="$PLUGIN_DIR/manifest.json"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/sorakey"
TARGET_DIR="$CACHE_DIR/target"
LIB_DIR="$HOME/.local/lib/sorakey"
BIN="$HOME/.local/bin/sorakey"
REPO="sandeshrai00/soraKey"
SHARE="$HOME/.local/share/sorakey"
STAMP="$SHARE/.bundled-packs"

mkdir -p "$CACHE_DIR" "$LIB_DIR" "$(dirname "$BIN")"

# Sync bundled soundpacks (plugin dir) -> share dir, where the daemon
# actually reads them from. `sora-install` copies packs once with `cp -rn`;
# without this step, pack changes delivered by `plugin update` would sit in
# the plugin dir forever while the daemon plays stale copies.
# Prints nothing on success unless something changed, when SYNC_LINE carries
# the machine-readable last line the panel notifies on. Warnings go to
# stderr only — a sync failure must never break the binary flow below.
# User-imported packs (never bundled, never stamped) are never touched.
packs_changed=0
SYNC_LINE=""
sync_soundpacks() {
  local src_dir="$DAEMON_DIR/soundpacks/keyboard"
  [[ -d "$src_dir" ]] || { echo "soundpacks sync warning: $src_dir missing" >&2; return 0; }
  mkdir -p "$SHARE/soundpacks/keyboard"
  local updated=0 removed=0
  local stamp_tmp; stamp_tmp=$(mktemp)
  trap 'rm -f "$stamp_tmp"' RETURN
  local src id dst
  for src in "$src_dir"/*/; do
    [[ -d "$src" ]] || continue
    id=$(basename "$src")
    dst="$SHARE/soundpacks/keyboard/$id"
    if [[ ! -d "$dst" ]] || ! diff -qr "$src" "$dst" >/dev/null 2>&1; then
      rm -rf "$dst"
      cp -r "$src" "$dst" || { echo "soundpacks sync warning: copy failed for $id" >&2; continue; }
      updated=$((updated + 1))
    fi
    echo "$id" >> "$stamp_tmp"
  done
  if [[ -f "$STAMP" ]]; then
    local old_id
    while read -r old_id _; do
      # STAMP is machine-written, but a hand-edited/planted line must never
      # steer an rm -rf: plain single-level dir names only.
      case "$old_id" in
        ""|*/*|*..*|-*) echo "soundpacks sync warning: skipping suspect stamp entry '$old_id'" >&2; continue ;;
      esac
      if [[ ! -d "$src_dir/$old_id" ]] && [[ -d "$SHARE/soundpacks/keyboard/$old_id" ]]; then
        rm -rf -- "$SHARE/soundpacks/keyboard/$old_id" && removed=$((removed + 1))
      fi
    done < "$STAMP" || true
  fi
  mv "$stamp_tmp" "$STAMP"
  if [[ "$updated" != 0 || "$removed" != 0 ]]; then
    packs_changed=1
    SYNC_LINE="soundpacks synced ($updated updated, $removed removed)"
  fi
  return 0
}

if ! version="$(python3 -c "import json;print(json.load(open('$MANIFEST'))['version'])" 2>/dev/null)"; then
  # A corrupt manifest used to hide as version 0.0.0 ("no prebuilt, building
  # from source") with no diagnostic — say so, then keep the safe fallback.
  echo "sora-build: cannot parse version from $MANIFEST — treating as 0.0.0 (no prebuilt will match)" >&2
  version="0.0.0"
fi
# Strict TOML parse (same form as release.yml): the old grep matched any
# first `version` line and broke on reorder, comments, or `version="x"`.
cargo_version="$(python3 -c 'import tomllib,sys;print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$DAEMON_DIR/Cargo.toml" 2>/dev/null || echo "")"
# manifest and daemon versions must agree: the release gate proves
# tag == manifest, so a manifest/daemon mismatch means no release can
# vouch for this source — build locally instead of trusting a prebuilt.
versions_match=0
[[ -n "$cargo_version" && "$cargo_version" == "$version" ]] && versions_match=1
arch="$(uname -m)"
case "$arch" in x86_64|aarch64) ;; *) arch="x86_64";; esac
asset="sorakey-${arch}"

# source hash for staleness
source_id=""
if command -v sha256sum >/dev/null 2>&1; then
  source_id=$( { find "$DAEMON_DIR" -path "$DAEMON_DIR/target" -prune -o \( -name "Cargo.toml" -o -name "Cargo.lock" -o -name "*.rs" \) -print0;
                 printf '%s\0' "$PLUGIN_DIR/rust-toolchain.toml"; } | sort -z | xargs -0 cat 2>/dev/null | sha256sum | cut -d' ' -f1)
  if [[ -f "$LIB_DIR/source.sha256" ]] && [[ "$(cat "$LIB_DIR/source.sha256" 2>/dev/null)" == "$source_id" ]] && [[ -x "$BIN" ]]; then
    # Binary is current: only packs may have moved. Skip the (identical)
    # prebuilt download below; the sync line alone restarts the daemon.
    sync_soundpacks
    if [[ "$packs_changed" == 0 ]]; then
      echo "sorakey up to date (source $source_id)"
    else
      echo "$SYNC_LINE"
    fi
    exit 0
  fi
fi

# only trust release if the COMPILED source matches the tagged commit.
# Soundpacks are data, not code: the binary never embeds them (source_id
# above doesn't hash them either), so pack-only changes must not reject an
# otherwise matching prebuilt. Docs promise this (docs/dev/relse.md); the
# ':!daemon/soundpacks' exclusions below are what actually honors it.
release_matches_source() {
  command -v git >/dev/null 2>&1 || return 1
  local dirty tag_commit
  # manifest.json is deliberately NOT a gate path: only its `version` field
  # matters for trust (download URL + versions_match + CI's tag==manifest
  # check), so a description edit must not force a source build.
  # Cargo.lock needs no explicit path either: it is tracked inside daemon/,
  # so lock changes trip the diff below like any other source change.
  dirty=$(git -C "$PLUGIN_DIR" status --porcelain --untracked-files=normal -- daemon rust-toolchain.toml ':!daemon/soundpacks' 2>/dev/null) || return 1
  [[ -z "$dirty" ]] || return 1
  tag_commit=$(git -C "$PLUGIN_DIR" rev-parse "refs/tags/v${version}^{commit}" 2>/dev/null) || return 1
  git -C "$PLUGIN_DIR" diff --quiet "$tag_commit" HEAD -- daemon rust-toolchain.toml ':!daemon/soundpacks' 2>/dev/null || return 1
}

# gh can verify only when authenticated
gh_can_verify() {
  command -v gh >/dev/null 2>&1 || return 1
  [[ -n "${GH_TOKEN:-}" ]] && return 0
  GH_PROMPT_DISABLED=1 gh auth status --active >/dev/null 2>&1
}

try_download_prebuilt() {
  release_matches_source || return 1
  if [[ "$versions_match" != 1 ]]; then
    echo "manifest ($version) != daemon Cargo.toml ($cargo_version) — building from source" >&2
    return 1
  fi
  command -v curl >/dev/null 2>&1 || return 1
  command -v sha256sum >/dev/null 2>&1 || return 1
  local url="https://github.com/$REPO/releases/download/v${version}/${asset}"
  local sums="https://github.com/$REPO/releases/download/v${version}/SHA256SUMS"
  local tmp
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' RETURN
  echo "Trying verified prebuilt $url ..."
  # Capture HTTP codes (curl -w prints even on -f failure) so the failure
  # says which it was: dead network, missing release, or missing checksums.
  # -f still rejects error bodies; the codes below only route the message.
  local asset_code sums_code
  asset_code=$(curl --proto '=https' --tlsv1.2 -fsSL --max-time 120 -o "$tmp/$asset" "$url" -w '%{http_code}' 2>/dev/null) || true
  sums_code=$(curl --proto '=https' --tlsv1.2 -fsSL --max-time 30 -o "$tmp/SHA256SUMS" "$sums" -w '%{http_code}' 2>/dev/null) || true
  if [[ "$asset_code" == "200" && "$sums_code" == "200" ]]; then
    # normalize SHA256SUMS, then verify ONLY the downloaded asset.
    # (The file lists every arch; sha256sum -c over the whole file fails
    # on the binaries we didn't download, rejecting a good prebuilt.)
    sed -i "s|dist/||g; s|\*||g" "$tmp/SHA256SUMS" 2>/dev/null || true
    expected=$(awk -v a="$asset" '$2 == a {print $1; exit}' "$tmp/SHA256SUMS" 2>/dev/null)
    actual=$(sha256sum "$tmp/$asset" 2>/dev/null | awk '{print $1}')
    if [[ -n "$expected" && "$expected" == "$actual" ]]; then
      if gh_can_verify; then
        # Exit code stays the signal (output wording is not a contract);
        # the captured text only explains the warning below.
        att_out=$(GH_PROMPT_DISABLED=1 gh attestation verify "$tmp/$asset" --repo "$REPO" \
             --cert-identity-regex "https://github.com/$REPO/.github/workflows/release.*" \
             --deny-self-hosted-runners 2>&1) && att_rc=0 || att_rc=$?
        if (( att_rc == 0 )); then
          install -m 755 "$tmp/$asset" "$BIN" || return 1
          [[ -n "$source_id" ]] && echo "$source_id" > "$LIB_DIR/source.sha256"
          rm -rf "$tmp"
          echo "Installed verified prebuilt $version $arch (attested)"
          return 0
        fi
        # attestation failed — fall back to source build
        echo "warning: attestation failed (${att_out:0:200}) — building from source" >&2
        rm -rf "$tmp"
        return 1
      fi
      # no attestation possible — checksum already passed
      install -m 755 "$tmp/$asset" "$BIN" || return 1
      [[ -n "$source_id" ]] && echo "$source_id" > "$LIB_DIR/source.sha256"
      rm -rf "$tmp"
      echo "Installed prebuilt $version $arch (release checksum verified; attestation skipped — gh not logged in, run 'gh auth login' for the attested path)"
      return 0
    fi
    echo "prebuilt checksum mismatch for $asset (expected $expected, got $actual) — building from source" >&2
  else
    case "$asset_code" in
      000) echo "prebuilt download failed: no network / DNS / TLS route to github.com" >&2 ;;
      404) echo "prebuilt download failed: no v${version} release assets (release missing?)" >&2 ;;
      200) echo "prebuilt download failed: binary ok but checksum file missing (HTTP $sums_code)" >&2 ;;
      *) echo "prebuilt download failed: HTTP $asset_code" >&2 ;;
    esac
  fi
  rm -rf "$tmp" 2>/dev/null || true
  return 1
}

# Binary is stale or missing: sync packs first so a rebuilt daemon never
# starts against stale sound files, then resolve the binary as before.
sync_soundpacks

if [[ "${SORAKEY_BUILD_FROM_SOURCE:-}" != "1" ]]; then
  if try_download_prebuilt; then
    if [[ "$packs_changed" == 1 ]]; then echo "$SYNC_LINE"; fi
    exit 0
  fi
  echo "No usable prebuilt for this source (no release yet, or source moved past the tag) — building from source"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found — install rustup (pacman -S rustup or https://rustup.rs) then re-run" >&2
  exit 1
fi

# build outside plugin dir
export SOURCE_DATE_EPOCH="$(git -C "$PLUGIN_DIR" log -1 --format=%ct 2>/dev/null || date +%s)"
export CARGO_INCREMENTAL=0
export CARGO_TERM_QUIET=true
cargo build --locked --release --manifest-path "$DAEMON_DIR/Cargo.toml" --target-dir "$TARGET_DIR"
install -m 755 "$TARGET_DIR/release/sorakey" "$BIN"
[[ -n "$source_id" ]] && echo "$source_id" > "$LIB_DIR/source.sha256"
echo "Built and installed $BIN"
if [[ "$packs_changed" == 1 ]]; then echo "$SYNC_LINE"; fi