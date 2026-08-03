#!/usr/bin/env bash
# ProofKiosk — one-command devnet/localnet setup.
#
# Creates everything rung 1 needs and prints a paste-ready config block:
#   (a) a running localnet validator, OR a devnet target
#   (b) a test SPL mint (6 decimals, USDC-like)
#   (c) separate merchant and customer wallets, with test tokens only on customer
#   (d) a durable nonce account for kiosk-attest
#   (e) canonical ZeroClaw `[[plugins.entries]]` config
#
# Usage:
#   MODE=localnet ./scripts/devnet-setup.sh   # default: local test validator
#   MODE=devnet   ./scripts/devnet-setup.sh   # target public devnet (airdrops SOL)
#   ./scripts/devnet-setup.sh --validate-layout  # CI-safe root/layout probe
#   ./scripts/devnet-setup.sh --validate-inputs  # CI-safe catalog/env probe
#
# Requires the Solana CLI (`solana`, `solana-keygen`) and `spl-token`.
# Nothing here touches mainnet. All keys are throwaway test keys under ./.devnet.

set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
MODE="${MODE:-localnet}"
DECIMALS=6
MAX_AMOUNT_USDC="10"
MINT_AMOUNT="${MINT_AMOUNT:-1000}"        # test tokens minted to the customer
# Emitted into BOTH plugin config blocks: kiosk-charge prices from it and
# kiosk-watch re-derives the gating amount from it, so they must not drift.
PRICE_LIST="${PRICE_LIST:-cold_drink:1.5, snack:0.75}"
PAYMENT_ITEM="${PAYMENT_ITEM:-cold_drink}"
WORKDIR="$ROOT/.devnet"
MERCHANT_WALLET="$WORKDIR/merchant.json"
CUSTOMER_WALLET="$WORKDIR/customer.json"
NONCE_KEYPAIR="$WORKDIR/nonce-account.json"
REFERENCE_KEYPAIR="$WORKDIR/payment-reference.json"
VALIDATOR_PID=""

log()  { printf '\033[1;36m[setup]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[setup] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# A side-effect-free layout probe used by CI. Keeping this path ahead of every
# external-tool check makes it safe to run on machines without the Solana CLI,
# and running it from another cwd proves all repository paths are root-based.
case "${1:-}" in
  --validate-layout)
    [[ "$#" -eq 1 ]] || die "--validate-layout accepts no additional arguments"
    [[ -f "$ROOT/scripts/check-config.sh" ]] || die "repository root is missing scripts/check-config.sh"
    [[ -d "$ROOT/config" && -d "$ROOT/plugins" ]] || die "repository root is missing config/plugins"
    printf '%s\n' "$ROOT"
    exit 0
    ;;
  --validate-inputs)
    [[ "$#" -eq 1 ]] || die "--validate-inputs accepts no additional arguments"
    VALIDATE_INPUTS=1
    ;;
  "") ;;
  *) die "unknown argument '$1' (supported: --validate-layout, --validate-inputs)" ;;
esac

PAYMENT_AMOUNT=""
PAYMENT_MATCHES=0
SEEN_PRICE_KEYS=()
SEEN_PRICE_KEY_COUNT=0
[[ "$PAYMENT_ITEM" =~ ^[A-Za-z0-9_-]{1,64}$ ]] \
  || die "PAYMENT_ITEM must use 1 to 64 ASCII letters, digits, '_' or '-'"
[[ "$PRICE_LIST" == *:* ]] || die "PRICE_LIST must contain at least one item:price entry"
[[ "${#PRICE_LIST}" -le 1024 ]] || die "PRICE_LIST exceeds the plugins' 1024-byte limit"

trim_whitespace() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

# Mirror kiosk-core's fixed-point grammar for this generated six-decimal mint.
# The early whole-part comparison bounds the later shell arithmetic, so a huge
# user string cannot wrap before being compared with max_amount_usdc=10.
price_to_base_units() {
  local decimal="$1" whole fraction whole_len cap_whole_len raw
  [[ "$decimal" =~ ^(0|[1-9][0-9]*)(\.[0-9]{1,6})?$ ]] || return 1
  whole="${decimal%%.*}"
  if [[ "$decimal" == *.* ]]; then
    fraction="${decimal#*.}"
  else
    fraction=""
  fi
  while [[ "${#fraction}" -lt "$DECIMALS" ]]; do
    fraction="${fraction}0"
  done
  whole_len="${#whole}"
  cap_whole_len="${#MAX_AMOUNT_USDC}"
  if [[ "$whole_len" -gt "$cap_whole_len" \
      || ( "$whole_len" -eq "$cap_whole_len" && "$whole" > "$MAX_AMOUNT_USDC" ) \
      || ( "$whole" == "$MAX_AMOUNT_USDC" && "$fraction" != "000000" ) ]]; then
    return 1
  fi
  raw=$((10#$whole * 1000000 + 10#$fraction))
  [[ "$raw" -gt 0 && "$raw" -le 10000000 ]] || return 1
  printf '%s' "$raw"
}

IFS=',' read -r -a PRICE_ENTRIES <<<"$PRICE_LIST"
for entry in "${PRICE_ENTRIES[@]}"; do
  key="${entry%%:*}"
  key="$(trim_whitespace "$key")"
  value="${entry#*:}"
  value="$(trim_whitespace "$value")"
  [[ "$entry" == *:* && "$key" =~ ^[A-Za-z0-9_-]{1,64}$ ]] \
    || die "invalid PRICE_LIST entry '$entry' (want item:positive decimal)"
  for ((seen_index = 0; seen_index < SEEN_PRICE_KEY_COUNT; seen_index++)); do
    seen_key="${SEEN_PRICE_KEYS[$seen_index]}"
    [[ "$seen_key" != "$key" ]] || die "duplicate PRICE_LIST item '$key'"
  done
  SEEN_PRICE_KEYS[$SEEN_PRICE_KEY_COUNT]="$key"
  SEEN_PRICE_KEY_COUNT=$((SEEN_PRICE_KEY_COUNT + 1))
  raw_amount="$(price_to_base_units "$value")" \
    || die "invalid PRICE_LIST price '$value' for '$key' (want 0 < price <= $MAX_AMOUNT_USDC with at most $DECIMALS places)"
  if [[ "$key" == "$PAYMENT_ITEM" ]]; then
    PAYMENT_AMOUNT="$value"
    PAYMENT_MATCHES=$((PAYMENT_MATCHES + 1))
  fi
done
[[ "$PAYMENT_MATCHES" -eq 1 && "$PAYMENT_AMOUNT" =~ ^(0|[1-9][0-9]*)(\.[0-9]+)?$ \
    && "$PAYMENT_AMOUNT" != "0" ]] \
  || die "PAYMENT_ITEM '$PAYMENT_ITEM' needs a positive decimal entry in PRICE_LIST"

if [[ "${VALIDATE_INPUTS:-0}" == "1" ]]; then
  printf 'validated %s price(s); %s=%s (%s base units)\n' \
    "$SEEN_PRICE_KEY_COUNT" "$PAYMENT_ITEM" "$PAYMENT_AMOUNT" \
    "$(price_to_base_units "$PAYMENT_AMOUNT")"
  exit 0
fi

command -v solana        >/dev/null || die "solana CLI not found — https://docs.solanalabs.com/cli/install"
command -v solana-keygen >/dev/null || die "solana-keygen not found (part of the Solana CLI)"
command -v spl-token     >/dev/null || die "spl-token not found — cargo install spl-token-cli"

mkdir -p "$WORKDIR"

cleanup() {
  if [[ -n "$VALIDATOR_PID" ]] && kill -0 "$VALIDATOR_PID" 2>/dev/null; then
    log "leaving solana-test-validator running (pid $VALIDATOR_PID); kill it with: kill $VALIDATOR_PID"
  fi
}
trap cleanup EXIT

# ── (a) endpoint ──────────────────────────────────────────────────────────────
case "$MODE" in
  localnet)
    RPC_URL="http://127.0.0.1:8899"
    if ! solana cluster-version --url "$RPC_URL" >/dev/null 2>&1; then
      command -v solana-test-validator >/dev/null || die "solana-test-validator not found"
      log "starting solana-test-validator (ledger in $WORKDIR/ledger)…"
      nohup solana-test-validator --quiet --ledger "$WORKDIR/ledger" \
        >"$WORKDIR/validator.log" 2>&1 &
      VALIDATOR_PID="$!"
      printf '%s\n' "$VALIDATOR_PID" >"$WORKDIR/validator.pid"
      # Wait for RPC to answer instead of a blind sleep.
      for _ in $(seq 1 30); do
        solana cluster-version --url "$RPC_URL" >/dev/null 2>&1 && break
        sleep 1
      done
      solana cluster-version --url "$RPC_URL" >/dev/null 2>&1 || die "validator did not come up"
    else
      log "reusing already-running local validator at $RPC_URL"
    fi
    ;;
  devnet)
    RPC_URL="https://api.devnet.solana.com"
    log "targeting public devnet"
    ;;
  *) die "MODE must be 'localnet' or 'devnet' (got '$MODE')" ;;
esac

# ── merchant wallet ─────────────────────────────────────────────────────────
if [[ ! -f "$MERCHANT_WALLET" ]]; then
  log "generating throwaway merchant wallet → $MERCHANT_WALLET"
  solana-keygen new --no-bip39-passphrase --silent --outfile "$MERCHANT_WALLET" >/dev/null
fi
if [[ ! -f "$CUSTOMER_WALLET" ]]; then
  log "generating throwaway customer wallet → $CUSTOMER_WALLET"
  solana-keygen new --no-bip39-passphrase --silent --outfile "$CUSTOMER_WALLET" >/dev/null
fi
MERCHANT="$(solana-keygen pubkey "$MERCHANT_WALLET")"
CUSTOMER="$(solana-keygen pubkey "$CUSTOMER_WALLET")"

log "funding merchant and customer with SOL (rent + transaction fees)…"
solana airdrop 2 "$MERCHANT" --url "$RPC_URL" >/dev/null 2>&1 || log "airdrop failed/limited — top up $MERCHANT manually if needed"
solana airdrop 2 "$CUSTOMER" --url "$RPC_URL" >/dev/null 2>&1 || log "airdrop failed/limited — top up $CUSTOMER manually if needed"

# ── (b) test SPL mint ─────────────────────────────────────────────────────────
log "creating test SPL mint ($DECIMALS decimals, USDC-like)…"
MINT="$(spl-token create-token --decimals "$DECIMALS" --url "$RPC_URL" --fee-payer "$MERCHANT_WALLET" --mint-authority "$MERCHANT" 2>/dev/null | awk '/Creating token/ {print $3}')"
[[ -n "$MINT" ]] || die "failed to create mint"

# ── (c) create both ATAs; mint only to the customer ──────────────────────────
log "creating merchant/customer token accounts and funding the customer…"
spl-token create-account "$MINT" --url "$RPC_URL" --fee-payer "$MERCHANT_WALLET" --owner "$MERCHANT" >/dev/null 2>&1 || true
spl-token create-account "$MINT" --url "$RPC_URL" --fee-payer "$MERCHANT_WALLET" --owner "$CUSTOMER" >/dev/null 2>&1 || true
CUSTOMER_ATA="$(spl-token address --verbose --token "$MINT" --owner "$CUSTOMER" --url "$RPC_URL" \
  | awk -F': ' '/^Associated token address:/ {print $2}')"
MERCHANT_ATA="$(spl-token address --verbose --token "$MINT" --owner "$MERCHANT" --url "$RPC_URL" \
  | awk -F': ' '/^Associated token address:/ {print $2}')"
[[ -n "$CUSTOMER_ATA" && -n "$MERCHANT_ATA" ]] || die "failed to derive token accounts"
spl-token mint "$MINT" "$MINT_AMOUNT" "$CUSTOMER_ATA" --url "$RPC_URL" --fee-payer "$MERCHANT_WALLET" --mint-authority "$MERCHANT_WALLET" >/dev/null

# A Solana Pay reference is an otherwise-unrelated public key attached as a
# read-only, non-signer account to the transfer. Generate a fresh one per setup
# so an earlier test payment can never satisfy a new order.
solana-keygen new --force --no-bip39-passphrase --silent --outfile "$REFERENCE_KEYPAIR" >/dev/null
REFERENCE="$(solana-keygen pubkey "$REFERENCE_KEYPAIR")"

# ── (d) durable nonce account for authenticated attestation genesis ─────────
if [[ ! -f "$NONCE_KEYPAIR" ]]; then
  log "generating durable nonce account keypair → $NONCE_KEYPAIR"
  solana-keygen new --no-bip39-passphrase --silent --outfile "$NONCE_KEYPAIR" >/dev/null
fi
NONCE_ACCOUNT="$(solana-keygen pubkey "$NONCE_KEYPAIR")"
if ! solana nonce-account "$NONCE_ACCOUNT" --url "$RPC_URL" >/dev/null 2>&1; then
  log "creating durable nonce account…"
  solana create-nonce-account "$NONCE_KEYPAIR" 0.01 \
    --url "$RPC_URL" \
    --keypair "$MERCHANT_WALLET" \
    --nonce-authority "$MERCHANT" >/dev/null
fi

# Machine-readable handoff for scripts/devnet-pay.mjs. This file contains only
# public test addresses and paths to throwaway local/devnet keypairs; it is
# ignored by Git with the rest of .devnet/.
PAYMENT_ENV="$WORKDIR/payment.env"
{
  printf 'export PROOFKIOSK_RPC_URL=%q\n' "$RPC_URL"
  printf 'export PROOFKIOSK_CUSTOMER_KEYPAIR=%q\n' "$CUSTOMER_WALLET"
  printf 'export PROOFKIOSK_MERCHANT=%q\n' "$MERCHANT"
  printf 'export PROOFKIOSK_MINT=%q\n' "$MINT"
  printf 'export PROOFKIOSK_REFERENCE=%q\n' "$REFERENCE"
  printf 'export PROOFKIOSK_ITEM=%q\n' "$PAYMENT_ITEM"
  printf 'export PROOFKIOSK_ITEM_ID=%q\n' "$PAYMENT_ITEM"
  printf 'export PROOFKIOSK_AMOUNT=%q\n' "$PAYMENT_AMOUNT"
  printf 'export PROOFKIOSK_MAX_AMOUNT=%q\n' "$MAX_AMOUNT_USDC"
  printf 'export PROOFKIOSK_TOKEN_DECIMALS=%q\n' "$DECIMALS"
  printf 'export PROOFKIOSK_PRICE_LIST=%q\n' "$PRICE_LIST"
  printf 'export PROOFKIOSK_NONCE_ACCOUNT=%q\n' "$NONCE_ACCOUNT"
  printf 'export PROOFKIOSK_CONFIG=%q\n' "$WORKDIR/zeroclaw.toml"
} >"$PAYMENT_ENV"

# ── (e) write one canonical, paste-ready config ───────────────────────────────
CONFIG_FILE="$WORKDIR/zeroclaw.toml"
cat >"$CONFIG_FILE" <<EOF
[plugins]
enabled = true
auto_discover = true

[[plugins.entries]]
name = "kiosk-charge"

[plugins.entries.config]
merchant_address = "$MERCHANT"
usdc_mint        = "$MINT"
token_decimals   = "$DECIMALS"
price_list       = "$PRICE_LIST"
max_amount_usdc  = "$MAX_AMOUNT_USDC"
label            = "Kiosk 01 (test)"

[[plugins.entries]]
name = "kiosk-watch"

[plugins.entries.config]
rpc_url          = "$RPC_URL"
merchant_address = "$MERCHANT"
usdc_mint        = "$MINT"
token_decimals   = "$DECIMALS"
# Same catalog as above: this is the ONLY source of the amount the relay gates on.
price_list       = "$PRICE_LIST"
# The only signer whose fulfillment marker counts. MUST equal kiosk-attest's
# nonce_authority, or the bounded on-chain replay marker cannot authenticate.
device_authority = "$MERCHANT"
device_address   = "$NONCE_ACCOUNT"
device_id        = "kiosk-01"
payment_window_s = "900"
heartbeat_max_silence_s = "1800"
# Payment verification requires "finalized"; weaker commitments are refused.
finality         = "finalized"

[[plugins.entries]]
name = "kiosk-attest"

[plugins.entries.config]
rpc_url          = "$RPC_URL"
device_id        = "kiosk-01"
nonce_account    = "$NONCE_ACCOUNT"
nonce_authority  = "$MERCHANT"
allowed_metrics  = "temp_c:-40:85, humidity:0:100"
custody_mode     = "t1"
EOF

"$ROOT/scripts/check-config.sh" "$CONFIG_FILE" >/dev/null

cat <<EOF

✔ ProofKiosk test environment ready ($MODE)

Canonical TEST-ONLY ZeroClaw config written to:
  $CONFIG_FILE

The two price_list lines must stay identical and device_authority must equal
kiosk-attest's nonce_authority. This setup already checks that exact file.

EOF
cat "$CONFIG_FILE"
cat <<EOF

Merchant:       $MERCHANT
Merchant ATA:   $MERCHANT_ATA
Customer:       $CUSTOMER
Customer ATA:   $CUSTOMER_ATA  ($MINT_AMOUNT test tokens)
Test mint:      $MINT
Nonce account:  $NONCE_ACCOUNT
Payment ref:    $REFERENCE

Merchant keypair: $MERCHANT_WALLET
Customer keypair: $CUSTOMER_WALLET
  (throwaway localnet/devnet keys only; never use either on mainnet)

Next:
  ./scripts/check-config.sh $CONFIG_FILE
  source $PAYMENT_ENV
  npm run devnet:pay                            # reference-bearing test-token transfer
  # Then invoke kiosk_watch with reference=$REFERENCE and item_id=$PAYMENT_ITEM.
EOF
