#!/usr/bin/env bash
# Enforce release WASM component size budgets. The RPC plugins intentionally
# have larger budgets because they bundle the `waki` HTTP/TLS client.

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && /bin/pwd -P)"
TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/kiosk-size}"

rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2 \
  || { echo "wasm32-wasip2 target missing — rustup target add wasm32-wasip2" >&2; exit 1; }

printf '%-16s %10s %10s   %s\n' "PLUGIN" "SIZE" "BUDGET" "RESULT"
printf '%s\n' "-----------------------------------------------------------"
status=0
for entry in kiosk-charge:250 kiosk-watch:400 kiosk-attest:450; do
  p="${entry%%:*}"
  limit_kb="${entry##*:}"
  plugin_dir="$ROOT/plugins/$p"
  [ -d "$plugin_dir" ] || {
    printf '%-16s %10s %8sKB   %s\n' "$p" "missing" "$limit_kb" "FAIL"
    status=1
    continue
  }

  ( cd "$ROOT/plugins/$p" && CARGO_TARGET_DIR="$TARGET_DIR/$p" \
      cargo build --locked --quiet --target wasm32-wasip2 --release )
  wasm="$TARGET_DIR/$p/wasm32-wasip2/release/${p//-/_}.wasm"
  [ -f "$wasm" ] || {
    printf '%-16s %10s %8sKB   %s\n' "$p" "missing" "$limit_kb" "FAIL"
    status=1
    continue
  }

  bytes=$(stat -f%z "$wasm" 2>/dev/null || stat -c%s "$wasm")
  kb=$(( (bytes + 1023) / 1024 ))
  limit_bytes=$(( limit_kb * 1024 ))
  if [ "$bytes" -le "$limit_bytes" ]; then
    mark="PASS"
  else
    mark="FAIL"
    status=1
  fi
  printf '%-16s %8dKB %8dKB   %s\n' "$p" "$kb" "$limit_kb" "$mark"
done

if [ "$status" -ne 0 ]; then
  echo "WASM size budget exceeded or an artifact is missing." >&2
fi
exit $status
