#!/usr/bin/env bash
# ProofKiosk — cross-plugin config check.
#
# Three plugins, three independent `[plugins.*.config]` sections, and no way for
# one to read another's. Two of those sections have to agree or a safety
# property silently stops holding:
#
#   1. kiosk-watch.device_authority == kiosk-attest.nonce_authority
#      The nonce authority is the fee payer, and only required signer, of every
#      fulfillment marker kiosk-attest builds. If watch is looking for a
#      different signer, no marker ever authenticates — single-use delivery
#      stops working and a paid charge can be dispensed on every poll. Nothing
#      errors; it just quietly never fires.
#
#   2. kiosk-watch.price_list == kiosk-charge.price_list
#      Watch re-derives the expected amount from its own copy. A drifted entry
#      makes a real payment read as a mismatch — fail-closed, but the kiosk
#      simply never delivers and the reason is not obvious.
#
# Both are checked here because both are invisible at runtime. Usage:
#
#   ./scripts/check-config.sh [path/to/config.toml]      # default: ~/.zeroclaw/config.toml

set -euo pipefail

CONFIG="${1:-$HOME/.zeroclaw/config.toml}"
[ -f "$CONFIG" ] || { echo "no such config: $CONFIG" >&2; exit 2; }

# Read `key = "value"` from within a [section], ignoring comments. Empty if unset.
value_in() {
  awk -v section="$1" -v key="$2" '
    /^[[:space:]]*\[/ { in_section = ($0 ~ "^[[:space:]]*\\[" section "\\]") ; next }
    !in_section { next }
    { sub(/#.*/, "") }
    $1 == key {
      if (match($0, /"[^"]*"/)) print substr($0, RSTART + 1, RLENGTH - 2)
      exit
    }
  ' "$CONFIG"
}

fail=0
report() { printf '  %s %s\n' "$1" "$2"; }

echo "checking $CONFIG"

watch_authority=$(value_in "plugins.kiosk-watch.config" "device_authority")
attest_authority=$(value_in "plugins.kiosk-attest.config" "nonce_authority")

if [ -z "$watch_authority" ]; then
  report "✘" "kiosk-watch.device_authority is unset — payment verification will refuse to run"
  fail=1
elif [ -z "$attest_authority" ]; then
  report "✘" "kiosk-attest.nonce_authority is unset — no fulfillment marker can be built"
  fail=1
elif [ "$watch_authority" != "$attest_authority" ]; then
  report "✘" "device_authority != nonce_authority — markers will never authenticate, delivery is NOT single-use"
  report " " "  watch.device_authority  = $watch_authority"
  report " " "  attest.nonce_authority  = $attest_authority"
  fail=1
else
  report "✔" "device_authority == nonce_authority (single-use delivery can be enforced)"
fi

# Compare price lists as normalized sets, so ordering and spacing do not matter.
normalize_prices() { tr ',' '\n' <<<"$1" | tr -d ' ' | grep -v '^$' | sort; }

watch_prices=$(value_in "plugins.kiosk-watch.config" "price_list")
charge_prices=$(value_in "plugins.kiosk-charge.config" "price_list")

if [ -z "$watch_prices" ]; then
  report "✘" "kiosk-watch.price_list is unset — no item can be verified, so nothing can be delivered"
  fail=1
elif [ "$(normalize_prices "$watch_prices")" != "$(normalize_prices "$charge_prices")" ]; then
  report "✘" "price_list differs between kiosk-charge and kiosk-watch — real payments will read as mismatches"
  report " " "  charge = $charge_prices"
  report " " "  watch  = $watch_prices"
  fail=1
else
  report "✔" "price_list matches between kiosk-charge and kiosk-watch"
fi

if [ "$fail" -ne 0 ]; then
  echo "config check FAILED" >&2
  exit 1
fi
echo "config check passed"
