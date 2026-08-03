#!/usr/bin/env bash
# ABI/load smoke test against a feature-enabled ZeroClaw host. It uses an
# isolated config directory and never touches ~/.zeroclaw.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ZEROCLAW_BIN="${ZEROCLAW_BIN:-$ROOT/.build/zeroclaw-install/bin/zeroclaw}"

[[ -x "$ZEROCLAW_BIN" ]] || {
  echo "feature-enabled ZeroClaw not found at $ZEROCLAW_BIN" >&2
  echo "run ./scripts/install-pinned-zeroclaw.sh first" >&2
  exit 1
}
"$ZEROCLAW_BIN" plugin --help >/dev/null

SMOKE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/proofkiosk-host-smoke.XXXXXX")"
cleanup() {
  [[ -n "$SMOKE_DIR" && -d "$SMOKE_DIR" ]] && rm -rf -- "$SMOKE_DIR"
}
trap cleanup EXIT

zc() { "$ZEROCLAW_BIN" --config-dir "$SMOKE_DIR" "$@"; }

echo "[host-smoke] isolated config: $SMOKE_DIR"
zc config set --no-interactive plugins.enabled true >/dev/null
zc config set --no-interactive plugins.auto_discover true >/dev/null

"$ROOT/scripts/stage-plugin.sh"
for plugin in kiosk-charge kiosk-watch kiosk-attest; do
  echo "[host-smoke] install + load $plugin"
  zc plugin install "$ROOT/staged/$plugin" >/dev/null
  zc plugin info "$plugin" | grep -q "WASM"
done

# `plugin install` seeds each natural-key row. Writes through these exact paths
# prove the pinned `[[plugins.entries]]` schema is active for every component,
# rather than silently accepting documentation for a different host layout.
set_plugin_config() {
  local plugin="$1" key="$2" value="$3"
  zc config set --no-interactive "plugins.entries.$plugin.config.$key" "$value" >/dev/null
  zc config get "plugins.entries.$plugin.config.$key" >/dev/null
}

TEST_KEY="11111111111111111111111111111111"
TEST_RPC="http://127.0.0.1:8899"
set_plugin_config kiosk-charge merchant_address "$TEST_KEY"
set_plugin_config kiosk-charge usdc_mint "$TEST_KEY"
set_plugin_config kiosk-charge token_decimals "6"
set_plugin_config kiosk-charge price_list "cold_drink:1.5"
set_plugin_config kiosk-watch rpc_url "$TEST_RPC"
set_plugin_config kiosk-watch merchant_address "$TEST_KEY"
set_plugin_config kiosk-watch usdc_mint "$TEST_KEY"
set_plugin_config kiosk-watch token_decimals "6"
set_plugin_config kiosk-watch price_list "cold_drink:1.5"
set_plugin_config kiosk-watch device_authority "$TEST_KEY"
set_plugin_config kiosk-watch device_address "$TEST_KEY"
set_plugin_config kiosk-watch device_id "kiosk-01"
set_plugin_config kiosk-watch payment_window_s "900"
set_plugin_config kiosk-watch heartbeat_max_silence_s "1800"
set_plugin_config kiosk-watch finality "finalized"
set_plugin_config kiosk-attest rpc_url "$TEST_RPC"
set_plugin_config kiosk-attest device_id "kiosk-01"
set_plugin_config kiosk-attest nonce_account "$TEST_KEY"
set_plugin_config kiosk-attest nonce_authority "$TEST_KEY"
set_plugin_config kiosk-attest allowed_metrics "temp_c:-40:85"
set_plugin_config kiosk-attest custody_mode "t1"

zc config set --no-interactive sop.sops_dir "$ROOT/sops" >/dev/null
SOP_LIST="$(zc sop list)"
EXPECTED_SOPS=(
  proofkiosk-payment-loop
  proofkiosk-sensor-loop
  proofkiosk-heartbeat
)
LOADED_SOPS="$(printf '%s\n' "$SOP_LIST" \
  | awk '$1 ~ /^proofkiosk-/ && $2 ~ /^v[0-9]/ { print $1 }' \
  | LC_ALL=C sort)"
EXPECTED_SORTED="$(printf '%s\n' "${EXPECTED_SOPS[@]}" | LC_ALL=C sort)"
[[ "$LOADED_SOPS" == "$EXPECTED_SORTED" ]] || {
  echo "[host-smoke] expected exactly these loaded SOPs:" >&2
  printf '  %s\n' "${EXPECTED_SOPS[@]}" >&2
  echo "[host-smoke] pinned host reported:" >&2
  printf '%s\n' "$SOP_LIST" >&2
  exit 1
}
for sop_id in "${EXPECTED_SOPS[@]}"; do
  # Pinned CLI syntax is `sop validate [NAME]`; validating names one by one
  # cannot succeed vacuously when the configured directory loads zero SOPs.
  zc sop validate "$sop_id" >/dev/null
done

RUNTIME_RESULT="$SMOKE_DIR/charge-tool-result.json"
RUNTIME_WATCH_RESULT="$SMOKE_DIR/watch-tool-result.json"
HANDOFF_RESULT="$SMOKE_DIR/trusted-handoff-result.json"
RUNTIME_CONFIG="$SMOKE_DIR/proofkiosk-runtime.toml"
cat >"$RUNTIME_CONFIG" <<EOF
[[plugins.entries]]
name = "kiosk-charge"
[plugins.entries.config]
merchant_address = "$TEST_KEY"
usdc_mint = "$TEST_KEY"
token_decimals = "6"
price_list = "cold_drink:1.5"

[[plugins.entries]]
name = "kiosk-watch"
[plugins.entries.config]
merchant_address = "$TEST_KEY"
usdc_mint = "$TEST_KEY"
token_decimals = "6"
price_list = "cold_drink:1.5"
payment_window_s = "900"
EOF

PROOFKIOSK_SKIP_STAGE=1 \
PROOFKIOSK_MERCHANT="$TEST_KEY" \
PROOFKIOSK_MINT="$TEST_KEY" \
PROOFKIOSK_TOKEN_DECIMALS=6 \
PROOFKIOSK_PRICE_LIST=cold_drink:1.5 \
PROOFKIOSK_ITEM_ID=cold_drink \
PROOFKIOSK_HOST_OUTPUT="$RUNTIME_RESULT" \
PROOFKIOSK_WATCH_OUTPUT="$RUNTIME_WATCH_RESULT" \
  "$ROOT/scripts/exact-host-runtime-smoke.sh"

# Carry the actual host-direct charge ToolResult across the same trusted
# handoff used by the QR renderer. This catches ABI/output/config drift that
# fixture-only Node tests cannot see.
node "$ROOT/scripts/trusted-charge-handoff.mjs" \
  --input "$RUNTIME_RESULT" \
  --config "$RUNTIME_CONFIG" \
  --orders-dir "$SMOKE_DIR/orders" >"$HANDOFF_RESULT"

REFERENCE="$(node -e '
  const fs = require("node:fs");
  const value = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (typeof value.reference !== "string") process.exit(1);
  process.stdout.write(value.reference);
' "$HANDOFF_RESULT")"

# Carry the actual pinned-host paid Watch ToolResult through the immutable
# quote/economics check and exclusive claim. A second claim must fail before an
# actuator could run twice.
node "$ROOT/scripts/trusted-order-claim.mjs" \
  --reference "$REFERENCE" \
  --watch-result "$RUNTIME_WATCH_RESULT" \
  --driver-id exact-host-smoke \
  --orders-dir "$SMOKE_DIR/orders" >/dev/null
if node "$ROOT/scripts/trusted-order-claim.mjs" \
    --reference "$REFERENCE" \
    --watch-result "$RUNTIME_WATCH_RESULT" \
    --driver-id exact-host-smoke \
    --orders-dir "$SMOKE_DIR/orders" >/dev/null 2>&1; then
  echo "[host-smoke] duplicate trusted claim unexpectedly succeeded" >&2
  exit 1
fi

echo "[host-smoke] PASS — exact pinned host executed all components, crossed the trusted charge + paid-watch claim boundary, rejected a duplicate claim, and validated all SOPs"
