#!/usr/bin/env bash
# ProofKiosk — cross-plugin config check.
#
# Three plugins, three independent `[[plugins.entries]]` rows, and no way for one
# to read another's config. Several values have to agree or a safety property
# silently stops holding:
#
#   1. kiosk-watch.device_authority == kiosk-attest.nonce_authority
#      The nonce authority is the fee payer, and only required signer, of every
#      fulfillment marker kiosk-attest builds. If watch is looking for a
#      different signer, no marker ever authenticates — the bounded on-chain
#      replay barrier stops working. The trusted local order claim remains the
#      mandatory at-most-once actuator gate.
#
#   2. kiosk-watch.price_list == kiosk-charge.price_list
#      Watch re-derives the expected amount from its own copy. A drifted entry
#      makes a real payment read as a mismatch — fail-closed, but the kiosk
#      simply never delivers and the reason is not obvious.
#
# These are checked here because drift is otherwise invisible at runtime. Run
# this against the plaintext deployment TOML *before* ZeroClaw's config CLI
# encrypts secret-looking values; randomized ciphertext cannot be compared.
# Usage:
#
#   ./scripts/check-config.sh [path/to/config.toml]      # default: ~/.zeroclaw/config.toml

set -euo pipefail

CONFIG="${1:-$HOME/.zeroclaw/config.toml}"
[ -f "$CONFIG" ] || { echo "no such config: $CONFIG" >&2; exit 2; }

# Read `key = "value"` from the `[plugins.entries.config]` block belonging to
# the `[[plugins.entries]]` row named `plugin`. Empty if unset. This deliberately
# follows the exact ZeroClaw revision in `wit/UPSTREAM_REF`; the tempting
# `[plugins.<name>.config]`
# shape is ignored by the host and would leave every plugin unconfigured.
value_in() {
  awk -v plugin="$1" -v key="$2" '
    /^[[:space:]]*\[\[plugins\.entries\]\][[:space:]]*$/ {
      current = ""
      in_config = 0
      next
    }
    /^[[:space:]]*\[/ {
      in_config = ($0 ~ /^[[:space:]]*\[plugins\.entries\.config\][[:space:]]*$/ && current == plugin)
      next
    }
    !in_config && $1 == "name" {
      line = $0
      sub(/#.*/, "", line)
      if (match(line, /"[^"]*"/)) current = substr(line, RSTART + 1, RLENGTH - 2)
      next
    }
    !in_config { next }
    {
      line = $0
      sub(/#.*/, "", line)
    }
    $1 == key {
      if (match(line, /"[^"]*"/)) print substr(line, RSTART + 1, RLENGTH - 2)
      exit
    }
  ' "$CONFIG"
}

fail=0
report() { printf '  %s %s\n' "$1" "$2"; }

echo "checking $CONFIG"

if awk '
  /^[[:space:]]*\[\[plugins\.entries\]\][[:space:]]*$/ { in_entry = 1; next }
  in_entry && /^[[:space:]]*\[plugins\.entries\.config\][[:space:]]*$/ { in_config = 1; next }
  /^[[:space:]]*\[/ { in_config = 0 }
  in_config && $0 ~ /=[[:space:]]*"enc2:/ { found = 1 }
  END { exit(found ? 0 : 1) }
' "$CONFIG"; then
  report "✘" "plugin config contains ZeroClaw-encrypted values (enc2); randomized ciphertext cannot be compared offline"
  report " " "run this checker on the plaintext deployment TOML before importing/updating it with ZeroClaw"
  echo "config check FAILED" >&2
  exit 2
fi

entry_count() {
  awk -v plugin="$1" '
    /^[[:space:]]*\[\[plugins\.entries\]\][[:space:]]*$/ { in_entry = 1; next }
    /^[[:space:]]*\[/ { in_entry = 0 }
    in_entry && $1 == "name" {
      line = $0
      sub(/#.*/, "", line)
      if (match(line, /"[^"]*"/) && substr(line, RSTART + 1, RLENGTH - 2) == plugin) count++
    }
    END { print count + 0 }
  ' "$CONFIG"
}

for plugin in kiosk-charge kiosk-watch kiosk-attest; do
  count=$(entry_count "$plugin")
  if [ "$count" -ne 1 ]; then
    report "✘" "$plugin must have exactly one [[plugins.entries]] row (found $count)"
    fail=1
  fi
done

watch_authority=$(value_in "kiosk-watch" "device_authority")
attest_authority=$(value_in "kiosk-attest" "nonce_authority")

if [ -z "$watch_authority" ]; then
  report "✘" "kiosk-watch.device_authority is unset — payment verification will refuse to run"
  fail=1
elif [ -z "$attest_authority" ]; then
  report "✘" "kiosk-attest.nonce_authority is unset — no fulfillment marker can be built"
  fail=1
elif [ "$watch_authority" != "$attest_authority" ]; then
  report "✘" "device_authority != nonce_authority — the bounded on-chain replay marker will never authenticate"
  report " " "  watch.device_authority  = $watch_authority"
  report " " "  attest.nonce_authority  = $attest_authority"
  fail=1
else
  report "✔" "device_authority == nonce_authority (bounded on-chain replay markers can authenticate)"
fi

watch_device=$(value_in "kiosk-watch" "device_address")
attest_nonce=$(value_in "kiosk-attest" "nonce_account")
watch_device_id=$(value_in "kiosk-watch" "device_id")
attest_device_id=$(value_in "kiosk-attest" "device_id")
if [ -z "$watch_device" ] || [ "$watch_device" != "$attest_nonce" ]; then
  report "✘" "kiosk-watch.device_address must equal kiosk-attest.nonce_account"
  fail=1
elif [ -z "$watch_device_id" ] || [ "$watch_device_id" != "$attest_device_id" ]; then
  report "✘" "kiosk-watch.device_id must equal kiosk-attest.device_id"
  fail=1
else
  report "✔" "heartbeat device address + id match kiosk-attest"
fi

# Compare price lists as normalized sets, so ordering and spacing do not matter.
normalize_prices() { tr ',' '\n' <<<"$1" | tr -d ' ' | grep -v '^$' | sort; }

watch_prices=$(value_in "kiosk-watch" "price_list")
charge_prices=$(value_in "kiosk-charge" "price_list")

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

watch_merchant=$(value_in "kiosk-watch" "merchant_address")
charge_merchant=$(value_in "kiosk-charge" "merchant_address")
watch_mint=$(value_in "kiosk-watch" "usdc_mint")
charge_mint=$(value_in "kiosk-charge" "usdc_mint")
watch_decimals=$(value_in "kiosk-watch" "token_decimals")
charge_decimals=$(value_in "kiosk-charge" "token_decimals")
watch_rpc=$(value_in "kiosk-watch" "rpc_url")
attest_rpc=$(value_in "kiosk-attest" "rpc_url")
finality=$(value_in "kiosk-watch" "finality")
custody=$(value_in "kiosk-attest" "custody_mode")
payment_window=$(value_in "kiosk-watch" "payment_window_s")
heartbeat_window=$(value_in "kiosk-watch" "heartbeat_max_silence_s")

if [ -z "$watch_merchant" ] || [ "$watch_merchant" != "$charge_merchant" ]; then
  report "✘" "merchant_address differs between kiosk-charge and kiosk-watch"
  fail=1
else
  report "✔" "merchant_address matches between charge and watch"
fi
if [ -z "$watch_mint" ] || [ "$watch_mint" != "$charge_mint" ]; then
  report "✘" "usdc_mint differs between kiosk-charge and kiosk-watch"
  fail=1
else
  report "✔" "usdc_mint matches between charge and watch"
fi

if [ -z "$watch_decimals" ] || [ -z "$charge_decimals" ] || [ "$watch_decimals" != "$charge_decimals" ]; then
  report "✘" "token_decimals must be explicitly set and identical in kiosk-charge and kiosk-watch"
  fail=1
elif ! [[ "$watch_decimals" =~ ^([0-9]|1[0-8])$ ]]; then
  report "✘" "token_decimals must be an integer from 0 to 18"
  fail=1
else
  report "✔" "token_decimals matches between charge and watch"
fi
if [ -z "$watch_rpc" ] || [ "$watch_rpc" != "$attest_rpc" ]; then
  report "✘" "rpc_url differs between kiosk-watch and kiosk-attest"
  fail=1
else
  report "✔" "rpc_url matches between watch and attest"
fi
if [ "$finality" != "finalized" ]; then
  report "✘" "kiosk-watch.finality must be finalized for payment verification"
  fail=1
else
  report "✔" "payment finality is finalized"
fi
if [ "$custody" != "t1" ]; then
  report "✘" "kiosk-attest.custody_mode must be t1"
  fail=1
else
  report "✔" "attestation custody mode is t1"
fi

valid_policy_seconds() {
  [[ "$1" =~ ^[0-9]+$ ]] && [ "$1" -ge 1 ] && [ "$1" -le 86400 ]
}
if ! valid_policy_seconds "$payment_window"; then
  report "✘" "kiosk-watch.payment_window_s must be 1..86400 seconds"
  fail=1
else
  report "✔" "payment replay window is operator-owned ($payment_window seconds)"
fi
if ! valid_policy_seconds "$heartbeat_window"; then
  report "✘" "kiosk-watch.heartbeat_max_silence_s must be 1..86400 seconds"
  fail=1
else
  report "✔" "heartbeat silence threshold is operator-owned ($heartbeat_window seconds)"
fi

if [ "$fail" -ne 0 ]; then
  echo "config check FAILED" >&2
  exit 1
fi
echo "config check passed"
