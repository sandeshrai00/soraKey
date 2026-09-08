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

mkdir -p "$CACHE_DIR" "$LIB_DIR" "$(dirname "$BIN")"

version="$(python3 -c "import json;print(json.load(open('$MANIFEST'))['version'])" 2>/dev/null || echo "0.0.0")"
cargo_version="$(grep -m1 '^version' "$DAEMON_DIR/Cargo.toml" 2>/dev/null | sed 's/.*"\(.*\)"/\1/' || echo "")"
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
    echo "sorakey up to date (source $source_id)"
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
  dirty=$(git -C "$PLUGIN_DIR" status --porcelain --untracked-files=normal -- daemon rust-toolchain.toml manifest.json ':!daemon/soundpacks' 2>/dev/null) || return 1
  [[ -z "$dirty" ]] || return 1
  tag_commit=$(git -C "$PLUGIN_DIR" rev-parse "refs/tags/v${version}^{commit}" 2>/dev/null) || return 1
  git -C "$PLUGIN_DIR" diff --quiet "$tag_commit" HEAD -- daemon rust-toolchain.toml manifest.json ':!daemon/soundpacks' 2>/dev/null || return 1
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
  if curl --proto '=https' --tlsv1.2 -fsSL --max-time 120 -o "$tmp/$asset" "$url" 2>/dev/null \
    && curl --proto '=https' --tlsv1.2 -fsSL --max-time 30 -o "$tmp/SHA256SUMS" "$sums" 2>/dev/null; then
    # normalize SHA256SUMS, then verify ONLY the downloaded asset.
    # (The file lists every arch; sha256sum -c over the whole file fails
    # on the binaries we didn't download, rejecting a good prebuilt.)
    sed -i "s|dist/||g; s|\*||g" "$tmp/SHA256SUMS" 2>/dev/null || true
    expected=$(awk -v a="$asset" '$2 == a {print $1; exit}' "$tmp/SHA256SUMS" 2>/dev/null)
    actual=$(sha256sum "$tmp/$asset" 2>/dev/null | awk '{print $1}')
    if [[ -n "$expected" && "$expected" == "$actual" ]]; then
      if gh_can_verify; then
        if GH_PROMPT_DISABLED=1 gh attestation verify "$tmp/$asset" --repo "$REPO" \
             --cert-identity-regex "https://github.com/$REPO/.github/workflows/release.*" \
             --deny-self-hosted-runners 2>/dev/null; then
          install -m 755 "$tmp/$asset" "$BIN" || return 1
          [[ -n "$source_id" ]] && echo "$source_id" > "$LIB_DIR/source.sha256"
          rm -rf "$tmp"
          echo "Installed verified prebuilt $version $arch (attested)"
          return 0
        fi
        # attestation failed — fall back to source build
        echo "warning: attestation failed — building from source" >&2
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
  fi
  rm -rf "$tmp" 2>/dev/null || true
  return 1
}

if [[ "${SORAKEY_BUILD_FROM_SOURCE:-}" != "1" ]]; then
  if try_download_prebuilt; then exit 0; fi
  echo "No usable prebuilt for this source (no release yet, or source moved past the tag) — building from source"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found — install rustup (pacman -S rustup or https://rustup.rs) then re-run" >&2
  exit 1
fi

# build outside plugin dir
export SOURCE_DATE_EPOCH=$(git -C "$PLUGIN_DIR" log -1 --format=%ct 2>/dev/null || date +%s)
export CARGO_INCREMENTAL=0
export CARGO_TERM_QUIET=true
cargo build --locked --release --manifest-path "$DAEMON_DIR/Cargo.toml" --target-dir "$TARGET_DIR"
install -m 755 "$TARGET_DIR/release/sorakey" "$BIN"
[[ -n "$source_id" ]] && echo "$source_id" > "$LIB_DIR/source.sha256"
echo "Built and installed $BIN"