#!/usr/bin/env bash
# Build the exact ZeroClaw revision whose WIT ABI is vendored in this repo.
# The stock release binary omits the WASM plugin runtime; this source build is
# therefore part of ProofKiosk's reproducible host, not an optional optimization.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/.build/zeroclaw-target}"

if [[ "${1:-}" == "--print-target-dir" ]]; then
  [[ "$#" -eq 1 ]] || {
    echo "usage: install-pinned-zeroclaw.sh [--print-target-dir]" >&2
    exit 2
  }
  printf '%s\n' "$TARGET_DIR"
  exit 0
fi
[[ "$#" -eq 0 ]] || {
  echo "usage: install-pinned-zeroclaw.sh [--print-target-dir]" >&2
  exit 2
}

UPSTREAM_REF="$(grep -E '^[0-9a-f]{40}$' "$ROOT/wit/UPSTREAM_REF" | head -1)"
[[ "$UPSTREAM_REF" =~ ^[0-9a-f]{40}$ ]] \
  || { echo "wit/UPSTREAM_REF does not contain one full git commit" >&2; exit 1; }

SHORT_REF="${UPSTREAM_REF:0:12}"
SOURCE_DIR="${ZEROCLAW_SOURCE_DIR:-$ROOT/.build/zeroclaw-$SHORT_REF}"
INSTALL_ROOT="${ZEROCLAW_INSTALL_ROOT:-$ROOT/.build/zeroclaw-install}"
FEATURES="${ZEROCLAW_FEATURES:-plugins-wasm-cranelift}"
REPOSITORY="https://github.com/zeroclaw-labs/zeroclaw.git"

command -v git >/dev/null || { echo "git is required" >&2; exit 1; }
command -v cargo >/dev/null || { echo "cargo is required" >&2; exit 1; }

if [[ ! -e "$SOURCE_DIR/.git" ]]; then
  if [[ -e "$SOURCE_DIR" ]]; then
    shopt -s nullglob dotglob
    SOURCE_ENTRIES=("$SOURCE_DIR"/*)
    shopt -u nullglob dotglob
    [[ "${#SOURCE_ENTRIES[@]}" -eq 0 ]] || {
      echo "refusing to initialize non-empty ZeroClaw source directory: $SOURCE_DIR" >&2
      exit 1
    }
  fi
  mkdir -p "$(dirname "$SOURCE_DIR")"
  git init -q "$SOURCE_DIR"
  git -C "$SOURCE_DIR" remote add origin "$REPOSITORY"
else
  # Check before fetch/checkout so this script never masks or overwrites local
  # modifications. An unborn but otherwise empty interrupted init is allowed.
  bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" --allow-any-ref "$SOURCE_DIR"
fi

echo "[zeroclaw] fetching pinned revision $UPSTREAM_REF"
git -C "$SOURCE_DIR" fetch -q --depth 1 origin "$UPSTREAM_REF"
git -C "$SOURCE_DIR" checkout -q --detach FETCH_HEAD

ACTUAL_REF="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
[[ "$ACTUAL_REF" == "$UPSTREAM_REF" ]] \
  || { echo "checked out $ACTUAL_REF, expected $UPSTREAM_REF" >&2; exit 1; }
bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" "$SOURCE_DIR"

# Cargo defaults a path dependency's target directory inside that dependency.
# Keep every generated artifact outside the source checkout so the post-build
# pristine-source check is meaningful and the exact source can be reused.
mkdir -p "$TARGET_DIR"
TARGET_DIR="$(cd "$TARGET_DIR" && pwd -P)"
SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd -P)"
case "$TARGET_DIR/" in
  "$SOURCE_DIR/"*)
    echo "CARGO_TARGET_DIR must be outside the pinned ZeroClaw source: $TARGET_DIR" >&2
    exit 1
    ;;
esac

echo "[zeroclaw] installing features: $FEATURES"
CARGO_TARGET_DIR="$TARGET_DIR" cargo install \
  --path "$SOURCE_DIR" \
  --locked \
  --force \
  --features "$FEATURES" \
  --root "$INSTALL_ROOT"

# Building must not rewrite the lockfile/source or leave even ignored files in
# the pinned checkout. The reusable Cargo target belongs outside SOURCE_DIR.
bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" "$SOURCE_DIR"

ZEROCLAW_BIN="$INSTALL_ROOT/bin/zeroclaw"
"$ZEROCLAW_BIN" plugin --help >/dev/null

echo "[zeroclaw] ready: $ZEROCLAW_BIN"
echo "[zeroclaw] revision: $ACTUAL_REF"
echo "Add it for this shell with:"
echo "  export PATH=\"$INSTALL_ROOT/bin:\$PATH\""
