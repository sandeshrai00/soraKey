#!/bin/bash
# sora-build.sh — prebuilt-only daemon install + bundled-pack sync.
#
# Policy: user machines NEVER compile. The daemon binary comes exclusively
# from the rolling `continuous` GitHub release, and only when that release
# records the byte-identical source this tree holds (content-hash match, not
# tag position — tags can move while assets stay stale). No match = hard
# failure with a reason, never a local cargo build.
#
# Test hooks (never set in production):
#   SORAKEY_RELEASE_BASE=file:///path/to/fake-release  — fetch fixtures
#   SORAKEY_ALLOW_SOURCE=1                              — dev-only cargo build
set -euo pipefail
PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DAEMON_DIR="$PLUGIN_DIR/daemon"
MANIFEST="$PLUGIN_DIR/manifest.json"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/sorakey"
TARGET_DIR="$CACHE_DIR/target"
LIB_DIR="$HOME/.local/lib/sorakey"
BIN="$HOME/.local/bin/sorakey"
REPO="sandeshrai00/soraKey"
RELEASE_BASE="${SORAKEY_RELEASE_BASE:-https://github.com/$REPO/releases/download/continuous}"
SHARE="$HOME/.local/share/sorakey"
STAMP="$SHARE/.bundled-packs"

mkdir -p "$CACHE_DIR" "$LIB_DIR" "$(dirname "$BIN")"

# Single-flight: the service freshness check and the panel setup can invoke
# this concurrently (shell start + auto-install overlap). Two pack syncs
# rm/cp the same dirs and two installs restart the daemon twice — harmless
# but stormy. The second runner waits on the lock instead, capped at 90s:
# a prebuilt fetch can legitimately take ~2 min (120s curl), but an
# indefinite stall (dead holder, stale lock) is worse than a loud skip —
# the caller retries (auto: bounded setupRetries; manual: unlimited taps).
# (No exec here: the fallback message must run in THIS shell after flock
# times out. Missing flock degrades to unlocked rather than failing.)
# ponytail: -w 90, not infinite flock — ceiling is one skipped-then-retried
# install under a wedged lock, acceptable vs hanging the panel forever.
if [ "${FLOCKED:-}" != 1 ] && command -v flock >/dev/null 2>&1; then
  env FLOCKED=1 flock -w 90 "$CACHE_DIR/build.lock" "$0" "$@"
  rc=$?
  if [ $rc -eq 0 ]; then exit 0; fi
  echo "sora-build: another install is still running after 90s — try again" >&2
  exit 1
fi

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
  echo "sora-build: cannot parse version from $MANIFEST" >&2
  exit 1
fi
# Strict TOML parse (same form as release.yml): the old grep matched any
# first `version` line and broke on reorder, comments, or `version="x"`.
cargo_version="$(python3 -c 'import tomllib,sys;print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$DAEMON_DIR/Cargo.toml" 2>/dev/null || echo "")"
# Manifest and daemon versions must agree: a mismatch means this tree is
# internally inconsistent, and no release can vouch for it.
if [[ -z "$cargo_version" || "$cargo_version" != "$version" ]]; then
  echo "sora-build: manifest ($version) != daemon Cargo.toml (${cargo_version:-unreadable}) — refusing to install from an inconsistent tree" >&2
  exit 1
fi
arch="$(uname -m)"
case "$arch" in x86_64|aarch64) ;; *) arch="x86_64";; esac
asset="sorakey-${arch}"

# Canonical source hash. MUST stay byte-identical to the pipeline in
# release.yml ("Record built source hash"): relative paths from the repo
# root, LC_ALL=C sort (sort order is locale-dependent — without the pin,
# identical trees hash differently on different boxes). Soundpacks are
# data, not code: the binary never embeds them, so they stay out.
source_id=""
if command -v sha256sum >/dev/null 2>&1; then
  source_id=$(cd "$PLUGIN_DIR" && { find daemon -path daemon/target -prune -o \( -name "Cargo.toml" -o -name "Cargo.lock" -o -name "*.rs" \) -print0;
                 printf '%s\0' "rust-toolchain.toml"; } | LC_ALL=C sort -z | xargs -0 cat 2>/dev/null | sha256sum | cut -d' ' -f1)
else
  echo "sora-build: sha256sum not found — cannot verify a prebuilt" >&2
  exit 1
fi
if [[ -f "$LIB_DIR/source.sha256" ]] && [[ "$(cat "$LIB_DIR/source.sha256" 2>/dev/null)" == "$source_id" ]] && [[ -x "$BIN" ]]; then
  # Binary is current: only packs may have moved. Skip the download below;
  # the sync line alone restarts the daemon.
  sync_soundpacks
  if [[ "$packs_changed" == 0 ]]; then
    echo "sorakey up to date (source $source_id)"
  else
    echo "$SYNC_LINE"
  fi
  exit 0
fi

# gh can verify only when authenticated
gh_can_verify() {
  command -v gh >/dev/null 2>&1 || return 1
  [[ -n "${GH_TOKEN:-}" ]] && return 0
  GH_PROMPT_DISABLED=1 gh auth status --active >/dev/null 2>&1
}

# Fetch a release file, printing "<http-code> <dest>" — file:// fixtures
# (tests) report 200 on success, 000 when missing.
fetch_release_file() {
  local name="$1" dest="$2"
  if [[ "$RELEASE_BASE" == file://* ]]; then
    if cp "${RELEASE_BASE#file://}/$name" "$dest" 2>/dev/null; then
      echo "200 $dest"
    else
      echo "000 $dest"
    fi
    return 0
  fi
  command -v curl >/dev/null 2>&1 || { echo "000 $dest"; return 0; }
  local code
  code=$(curl --proto '=https' --tlsv1.2 -fsSL --max-time 120 -o "$dest" "$RELEASE_BASE/$name" -w '%{http_code}' 2>/dev/null) || code="000"
  echo "$code $dest"
}

fail_no_prebuilt() {
  # $1 = reason line. The contract: explain which side is wrong and what
  # to do. Never fall through to a compile — user machines have no toolchain
  # by design, and a silent wrong binary is worse than a loud refusal.
  echo "sora-build: no prebuilt for source ${source_id:0:12} — $1" >&2
  echo "sora-build: Sorakey never builds from source on your machine. Update the plugin (fresh commits need ~10 min for CI to publish), then retry." >&2
  exit 1
}

try_download_prebuilt() {
  local tmp
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' RETURN
  local hash_code hash_file="$tmp/source.sha256"
  read -r hash_code _ < <(fetch_release_file "source.sha256" "$hash_file")
  case "$hash_code" in
    000) fail_no_prebuilt "cannot reach the prebuilt release (no network, or no continuous release published yet)" ;;
    404) fail_no_prebuilt "release has no source record yet (CI still building main?)" ;;
    200) ;;
    *) fail_no_prebuilt "source-record download failed (HTTP $hash_code)" ;;
  esac
  local built_source
  built_source=$(tr -d '[:space:]' < "$hash_file" 2>/dev/null)
  if [[ -z "$built_source" ]]; then
    fail_no_prebuilt "release source record is empty — CI artifact corrupt, report this"
  fi
  if [[ "$built_source" != "$source_id" ]]; then
    # Either side can be newer: local commits/CI lag, or (stale-asset bug
    # class) a release that predates this source. Both refuse loudly.
    fail_no_prebuilt "source mismatch (local ${source_id:0:12} != built ${built_source:0:12}) — local edits can never match (commit+push and wait for CI), otherwise wait for CI to publish this source"
  fi
  echo "Trying verified prebuilt $RELEASE_BASE/$asset ..."
  local url_code sums_code
  read -r url_code _ < <(fetch_release_file "$asset" "$tmp/$asset")
  read -r sums_code _ < <(fetch_release_file "SHA256SUMS" "$tmp/SHA256SUMS")
  if [[ "$url_code" == "200" && "$sums_code" == "200" ]]; then
    # normalize SHA256SUMS, then verify ONLY the downloaded asset.
    # (The file lists every arch; sha256sum -c over the whole file fails
    # on the binaries we didn't download, rejecting a good prebuilt.)
    sed -i "s|dist/||g; s|\*||g" "$tmp/SHA256SUMS" 2>/dev/null || true
    local expected actual
    expected=$(awk -v a="$asset" '$2 == a {print $1; exit}' "$tmp/SHA256SUMS" 2>/dev/null)
    actual=$(sha256sum "$tmp/$asset" 2>/dev/null | awk '{print $1}')
    if [[ -n "$expected" && "$expected" == "$actual" ]]; then
      if gh_can_verify; then
        # Exit code stays the signal (output wording is not a contract);
        # the captured text only explains the warning below.
        local att_out att_rc
        att_out=$(GH_PROMPT_DISABLED=1 gh attestation verify "$tmp/$asset" --repo "$REPO" \
             --cert-identity-regex "https://github.com/$REPO/.github/workflows/release.*" \
             --deny-self-hosted-runners 2>&1) && att_rc=0 || att_rc=$?
        if (( att_rc == 0 )); then
          install -m 755 "$tmp/$asset" "$BIN" || fail_no_prebuilt "cannot write $BIN (permissions?)"
          echo "$source_id" > "$LIB_DIR/source.sha256"
          rm -rf "$tmp"
          echo "Installed verified prebuilt $version $arch (attested)"
          return 0
        fi
        fail_no_prebuilt "attestation failed (${att_out:0:200}) — refusing an unattested binary"
      fi
      # no attestation possible — checksum already passed
      install -m 755 "$tmp/$asset" "$BIN" || fail_no_prebuilt "cannot write $BIN (permissions?)"
      echo "$source_id" > "$LIB_DIR/source.sha256"
      rm -rf "$tmp"
      echo "Installed prebuilt $version $arch (release checksum verified; attestation skipped — gh not logged in, run 'gh auth login' for the attested path)"
      return 0
    fi
    fail_no_prebuilt "prebuilt checksum mismatch for $asset (expected ${expected:-empty}, got ${actual:-empty}) — release artifact corrupt, report this"
  else
    case "$url_code" in
      000) fail_no_prebuilt "no network route to the release host" ;;
      404) fail_no_prebuilt "binary asset missing from the release (CI artifact incomplete?)" ;;
      200) fail_no_prebuilt "binary ok but checksum file missing (HTTP $sums_code)" ;;
      *) fail_no_prebuilt "binary download failed (HTTP $url_code)" ;;
    esac
  fi
}

# Dev-only escape hatch. Default off: user machines take this path never.
# Local uncommitted source can never match a CI prebuilt, so development
# builds set SORAKEY_ALLOW_SOURCE=1 explicitly.
if [[ "${SORAKEY_ALLOW_SOURCE:-}" == "1" ]]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "SORAKEY_ALLOW_SOURCE=1 but cargo not found — install rustup, then re-run" >&2
    exit 1
  fi
  sync_soundpacks
  export SOURCE_DATE_EPOCH="$(git -C "$PLUGIN_DIR" log -1 --format=%ct 2>/dev/null || date +%s)"
  export CARGO_INCREMENTAL=0
  export CARGO_TERM_QUIET=true
  cargo build --locked --release --manifest-path "$DAEMON_DIR/Cargo.toml" --target-dir "$TARGET_DIR"
  install -m 755 "$TARGET_DIR/release/sorakey" "$BIN"
  echo "$source_id" > "$LIB_DIR/source.sha256"
  echo "Built from source (dev bypass SORAKEY_ALLOW_SOURCE=1) and installed $BIN"
  if [[ "$packs_changed" == 1 ]]; then echo "$SYNC_LINE"; fi
  exit 0
fi

# Binary is stale or missing: sync packs first so a new daemon never
# starts against stale sound files, then resolve the binary — prebuilt or
# refusal, nothing in between.
sync_soundpacks
try_download_prebuilt
if [[ "$packs_changed" == 1 ]]; then echo "$SYNC_LINE"; fi
