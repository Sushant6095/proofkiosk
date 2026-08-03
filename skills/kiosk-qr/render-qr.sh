#!/usr/bin/env bash
# Validate a raw kiosk-charge ToolResult against operator config, persist the
# resulting order, then render its Solana Pay URL to a QR PNG. Host-side only.
# Never pass model prose or an arbitrary `solana:` URL to this boundary.
#
# Usage: render-qr.sh RAW_TOOL_RESULT.json CONFIG.toml [out.png]

set -euo pipefail

RAW_RESULT="${1:-}"
CONFIG="${2:-}"
OUT="${3:-kiosk-charge-qr.png}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ORDERS_DIR="${PROOFKIOSK_ORDERS_DIR:-$REPO_ROOT/.proofkiosk/orders}"

[ -f "$RAW_RESULT" ] && [ -f "$CONFIG" ] || {
  echo "usage: render-qr.sh RAW_TOOL_RESULT.json CONFIG.toml [out.png]" >&2
  exit 2
}

URL="$(node "$REPO_ROOT/scripts/trusted-charge-handoff.mjs" \
  --input "$RAW_RESULT" \
  --config "$CONFIG" \
  --orders-dir "$ORDERS_DIR" \
  --url-only)"

# QR image (skip gracefully if qrencode is absent — the tap-link still works).
if command -v qrencode >/dev/null 2>&1; then
  qrencode -o "$OUT" -s 8 -m 2 "$URL"
  echo "QR written: $OUT"
else
  echo "note: 'qrencode' not installed — skipping PNG (install: brew/apt install qrencode)" >&2
fi

# Tap-link fallback: this validated `solana:` URI is itself the tappable link — wallets
# (Phantom, Solflare, …) register the `solana:` scheme and open it directly with
# the payment pre-filled. No encoding or wrapper service needed.
echo "tap-link (opens a mobile wallet): $URL"
