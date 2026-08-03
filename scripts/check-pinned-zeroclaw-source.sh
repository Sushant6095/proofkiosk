#!/usr/bin/env bash
# Fail closed unless a ZeroClaw source tree is an isolated, pristine checkout.
# By default its HEAD must also match ProofKiosk's WIT-pinned revision.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
ALLOW_ANY_REF=0

if [[ "${1:-}" == "--allow-any-ref" ]]; then
  ALLOW_ANY_REF=1
  shift
fi

[[ "$#" -eq 1 ]] || {
  echo "usage: check-pinned-zeroclaw-source.sh [--allow-any-ref] SOURCE_DIR" >&2
  exit 2
}

SOURCE_DIR="$1"
[[ -d "$SOURCE_DIR" ]] || {
  echo "ZeroClaw source directory does not exist: $SOURCE_DIR" >&2
  exit 1
}

SOURCE_ROOT="$(git -C "$SOURCE_DIR" rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ZeroClaw source is not a Git checkout: $SOURCE_DIR" >&2
  exit 1
}
SOURCE_ROOT="$(cd "$SOURCE_ROOT" && pwd -P)"
SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd -P)"
[[ "$SOURCE_ROOT" == "$SOURCE_DIR" ]] || {
  echo "ZeroClaw source must be the checkout root (got $SOURCE_DIR; root is $SOURCE_ROOT)" >&2
  exit 1
}

TRACKED_STATE="$(git -C "$SOURCE_DIR" status --porcelain=v1 --untracked-files=no)"
[[ -z "$TRACKED_STATE" ]] || {
  echo "ZeroClaw checkout has tracked or staged changes:" >&2
  printf '%s\n' "$TRACKED_STATE" | sed -n '1,10p' >&2
  exit 1
}

# Do not let an upstream ignore rule hide generated or injected source. Cargo's
# target directory is deliberately kept outside this checkout by our callers.
UNTRACKED_COUNT="$(git -C "$SOURCE_DIR" ls-files --others -z | tr -cd '\0' | wc -c | tr -d '[:space:]')"
[[ "$UNTRACKED_COUNT" == "0" ]] || {
  echo "ZeroClaw checkout has $UNTRACKED_COUNT untracked/ignored file(s):" >&2
  git -C "$SOURCE_DIR" ls-files --others | sed -n '1,10p' >&2
  exit 1
}

if [[ "$ALLOW_ANY_REF" == "0" ]]; then
  UPSTREAM_REF="$(grep -E '^[0-9a-f]{40}$' "$ROOT/wit/UPSTREAM_REF" | head -1)"
  [[ "$UPSTREAM_REF" =~ ^[0-9a-f]{40}$ ]] || {
    echo "wit/UPSTREAM_REF does not contain one full Git commit" >&2
    exit 1
  }
  ACTUAL_REF="$(git -C "$SOURCE_DIR" rev-parse --verify HEAD 2>/dev/null)" || {
    echo "ZeroClaw checkout has no HEAD" >&2
    exit 1
  }
  [[ "$ACTUAL_REF" == "$UPSTREAM_REF" ]] || {
    echo "ZeroClaw source is at $ACTUAL_REF, expected $UPSTREAM_REF" >&2
    exit 1
  }
fi
