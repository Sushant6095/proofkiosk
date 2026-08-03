#!/usr/bin/env bash
# Execute all three compiled components through the exact pinned ZeroClaw
# WasmTool runtime. Unlike `plugin info`, this instantiates the component and
# calls its WIT execute export; kiosk-charge also proves host config injection
# replaces a caller-supplied `__config` object.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPSTREAM_REF="$(grep -E '^[0-9a-f]{40}$' "$ROOT/wit/UPSTREAM_REF" | head -1)"
SOURCE_DIR="${ZEROCLAW_SOURCE_DIR:-$ROOT/.build/zeroclaw-${UPSTREAM_REF:0:12}}"

[[ -d "$SOURCE_DIR/crates/zeroclaw-plugins" ]] || {
  echo "exact ZeroClaw source missing: run ./scripts/install-pinned-zeroclaw.sh" >&2
  exit 1
}
bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" "$SOURCE_DIR"

if [[ "${PROOFKIOSK_SKIP_STAGE:-0}" != "1" ]]; then
  "$ROOT/scripts/stage-plugin.sh"
fi

TEST_RELATIVE="crates/zeroclaw-plugins/tests/proofkiosk_runtime_e2e.rs"
TEST_TARGET="$SOURCE_DIR/$TEST_RELATIVE"
[[ ! -e "$TEST_TARGET" ]] || {
  echo "refusing to overwrite existing $TEST_TARGET" >&2
  exit 1
}
cleanup() {
  local status="$?"
  trap - EXIT
  if [[ "$TEST_TARGET" == "$SOURCE_DIR/$TEST_RELATIVE" ]]; then
    rm -f -- "$TEST_TARGET"
  else
    echo "refusing to clean unexpected exact-host test path: $TEST_TARGET" >&2
    status=1
  fi
  if ! bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" "$SOURCE_DIR"; then
    echo "ZeroClaw checkout was contaminated during exact-host execution" >&2
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT
cp "$ROOT/tests/exact-host-runtime/main.rs" "$TEST_TARGET"

# This generated test is the sole checkout delta allowed, only between the
# copy above and the EXIT trap. It must be byte-identical to the repository
# fixture; every other tracked, untracked, or ignored path is rejected.
TRACKED_STATE="$(git -C "$SOURCE_DIR" status --porcelain=v1 --untracked-files=no)"
UNTRACKED_STATE="$(git -C "$SOURCE_DIR" ls-files --others)"
[[ -z "$TRACKED_STATE" && "$UNTRACKED_STATE" == "$TEST_RELATIVE" ]] || {
  echo "ZeroClaw checkout changed outside the exact runtime test lifecycle" >&2
  exit 1
}
cmp -s "$ROOT/tests/exact-host-runtime/main.rs" "$TEST_TARGET" || {
  echo "generated exact-host test differs from the repository fixture" >&2
  exit 1
}

export PROOFKIOSK_CHARGE_WASM="$ROOT/staged/kiosk-charge/kiosk_charge.wasm"
export PROOFKIOSK_WATCH_WASM="$ROOT/staged/kiosk-watch/kiosk_watch.wasm"
export PROOFKIOSK_ATTEST_WASM="$ROOT/staged/kiosk-attest/kiosk_attest.wasm"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/.build/zeroclaw-target}"

cargo test \
  --locked \
  --manifest-path "$SOURCE_DIR/Cargo.toml" \
  -p zeroclaw-plugins \
  --features plugins-wasm-cranelift \
  --test proofkiosk_runtime_e2e \
  -- --nocapture
