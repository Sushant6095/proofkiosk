#!/usr/bin/env bash
# Fast, side-effect-free-on-the-repository regression checks for host scripts.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
FIXTURE="$(mktemp -d "${TMPDIR:-/tmp}/proofkiosk-host-infra.XXXXXX")"
cleanup() {
  [[ -n "$FIXTURE" && -d "$FIXTURE" ]] && rm -rf -- "$FIXTURE"
}
trap cleanup EXIT

fail() {
  echo "host infra regression failed: $*" >&2
  exit 1
}

# Execute outside the repository so a missing/relative ROOT cannot be hidden by
# the caller's cwd. This mode exits before checking CLIs or creating .devnet.
REPORTED_ROOT="$(cd "${TMPDIR:-/tmp}" && "$ROOT/scripts/devnet-setup.sh" --validate-layout)"
[[ "$REPORTED_ROOT" == "$ROOT" ]] || fail "devnet setup reported '$REPORTED_ROOT', expected '$ROOT'"

REPORTED_TARGET="$(cd "${TMPDIR:-/tmp}" && env -u CARGO_TARGET_DIR \
  "$ROOT/scripts/install-pinned-zeroclaw.sh" --print-target-dir)"
[[ "$REPORTED_TARGET" == "$ROOT/.build/zeroclaw-target" ]] \
  || fail "ZeroClaw installer target '$REPORTED_TARGET' is not outside its source checkout"

# Price/catalog validation is also side-effect free and runs before any Solana
# command. Exercise the precise cases that otherwise create a config both
# plugins refuse after chain accounts have already been mutated.
PRICE_LIST='cold_drink:10, snack:0.000001' PAYMENT_ITEM=cold_drink \
  "$ROOT/scripts/devnet-setup.sh" --validate-inputs >/dev/null
if PRICE_LIST='cold_drink:0.000000' PAYMENT_ITEM=cold_drink \
    "$ROOT/scripts/devnet-setup.sh" --validate-inputs >/dev/null 2>&1; then
  fail "zero-at-mint-precision price was accepted"
fi
if PRICE_LIST='cold_drink:1, snack:2, snack:3' PAYMENT_ITEM=cold_drink \
    "$ROOT/scripts/devnet-setup.sh" --validate-inputs >/dev/null 2>&1; then
  fail "duplicate non-selected catalog item was accepted"
fi
if PRICE_LIST='cold_drink:1, snack:10.000001' PAYMENT_ITEM=cold_drink \
    "$ROOT/scripts/devnet-setup.sh" --validate-inputs >/dev/null 2>&1; then
  fail "catalog price above max_amount_usdc was accepted"
fi

git init -q "$FIXTURE"
git -C "$FIXTURE" config user.name "ProofKiosk CI"
git -C "$FIXTURE" config user.email "proofkiosk-ci@example.invalid"
git -C "$FIXTURE" config commit.gpgsign false
printf 'build/\n' >"$FIXTURE/.gitignore"
printf 'clean\n' >"$FIXTURE/tracked.txt"
git -C "$FIXTURE" add .gitignore tracked.txt
git -C "$FIXTURE" commit -q -m fixture

bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" --allow-any-ref "$FIXTURE"

printf 'dirty\n' >>"$FIXTURE/tracked.txt"
if bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" --allow-any-ref "$FIXTURE" >/dev/null 2>&1; then
  fail "tracked source modification was accepted"
fi
git -C "$FIXTURE" restore tracked.txt

printf 'untracked\n' >"$FIXTURE/untracked.txt"
if bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" --allow-any-ref "$FIXTURE" >/dev/null 2>&1; then
  fail "untracked source file was accepted"
fi
rm -f -- "$FIXTURE/untracked.txt"

mkdir -p "$FIXTURE/build"
printf 'ignored but unsafe\n' >"$FIXTURE/build/generated.rs"
if bash "$ROOT/scripts/check-pinned-zeroclaw-source.sh" --allow-any-ref "$FIXTURE" >/dev/null 2>&1; then
  fail "ignored source contamination was accepted"
fi

echo "[host-infra] PASS — canonical root, external build target, and fail-closed checkout checks"
