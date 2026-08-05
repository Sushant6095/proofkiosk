# ProofKiosk local + Solana Devnet test runbook

Refreshed from repository base HEAD `7da49b41038d64e35d3e3082c9e7d62c85256f81` and the
ZeroClaw WIT pin `e112ce6b5ccdac9e1cb166bab217e730dd7e24c2`. Last audited:
2026-08-02.

This guide is deliberately honest about the boundary between a runnable test and a
simulated target. The separate-customer Solana Pay harness submits, finalizes, and
validates a reference-bearing test-token transfer; charge and watcher plugins are tested
independently around that rail. The
automatic relay and finalized attestation path is **not** complete and must not be
presented as working evidence.

## 1. What moves money—and what does not

| Component | Sends tokens? | Uses a private key? | What it really does today |
|---|---:|---:|---|
| `kiosk_charge` | No | No | Builds a Solana Pay URL using operator configuration. |
| Customer wallet | **Yes** | **Yes, inside the wallet** | Sends the fake Devnet SPL token to the merchant. |
| `kiosk_watch` | No | No | Reads RPC and checks a caller-supplied reference + item against operator-owned merchant, mint, price, finality, and time-window config. |
| `kiosk_attest` | No | No | Builds a bounded unsigned Solana durable-nonce message and reports `signature_required`. It does not sign or submit it. |
| External attestation signer | Would pay a Devnet SOL fee | Would hold the authority key | **Not implemented in this repository.** |
| Relay/SOP | No money | No | **Not runnable end to end. Do not connect an energized load.** |

Devnet SOL and the custom six-decimal token created below have **no real value**. The
custom token is USDC-like for testing; it is not Circle USDC and may appear as an unknown
token in a wallet.

Current code-backed hardening: charge references come from the host CSPRNG; payment
verdicts require `finalized`; watcher price and both time thresholds are operator config;
item-priced transfers require the exact versioned `PKPAY1` memo binding reference + item;
the ten-entry reference window scans/authenticates every candidate; payment and heartbeat
business verdicts are structured JSON; fresh nonce bootstrap and later chain heads
authenticate the expected System instruction, authority, device, and memo; heartbeat
ignores spoofed transactions; JSON-RPC validates envelope ids/version, applies a connect
timeout, and caps the body at two MiB (but has no overall response/read deadline); and
`kiosk_attest` rejects every custody mode except `t1`.

## 2. Current workstation blockers detected by the audit

Run these first:

```bash
cd /path/to/proofkiosk        # your checkout

rustc --version
zeroclaw --version
zeroclaw plugin --help
solana --version
spl-token --version
```

Observed on this workstation:

- The stock `zeroclaw 0.8.3` binary is pluginless. ProofKiosk now ships
  `scripts/install-pinned-zeroclaw.sh`, which builds exact commit
  `e112ce6b5ccdac9e1cb166bab217e730dd7e24c2` (source version 0.8.2) with
  `plugins-wasm-cranelift` into `.build/zeroclaw-install`.
- Rust/Cargo `1.97.1`, Solana CLI `4.1.1`, `spl-token-cli 5.6.1`, Node
  `24.10.0`, and the `wasm32-wasip2` target are present and executable.
- All 225 repository tests pass: 213 Rust tests (80 core / 19 charge / 76 watch /
  38 attest) plus 12 Node trusted-boundary tests. A separate exact pinned-host integration
  test passes. None of those contacts public Devnet, signs an attestation, or drives
  hardware.
- `qrencode`, `wasmtime`, and `wasm-tools` are absent. Only `qrencode` is useful for
  this manual flow; the host embeds its own WASM runtime after ZeroClaw is rebuilt.

Do not continue until `rustc --version` succeeds and `zeroclaw plugin --help` exists.

## 3. Sites used during the test

Use only Devnet/testnet pages and verify the `cluster=devnet` query string in explorer
links.

1. [Official Solana local installation](https://solana.com/docs/intro/installation)
2. [Official Solana cluster/RPC reference](https://solana.com/docs/references/clusters)
3. [Official Solana Devnet faucet](https://faucet.solana.com/)
4. [Solana Explorer on Devnet](https://explorer.solana.com/?cluster=devnet)
5. [Phantom: enable testnet mode](https://docs.phantom.com/developer-powertools/testnet-mode)
6. [Solana token creation](https://solana.com/docs/tokens/basics/create-mint)
7. [Solana token minting](https://solana.com/docs/tokens/basics/mint-tokens)
8. [Solana token transfers](https://solana.com/docs/tokens/basics/transfer-tokens)
9. [Official Solana Pay specification](https://github.com/solana-foundation/pay/blob/main/typescript/packages/solana-pay/docs/src/SPEC.md)
10. [Official Solana Pay merchant flow](https://github.com/solana-foundation/pay/blob/main/typescript/packages/solana-pay/docs/src/core/transfer-request/MERCHANT_INTEGRATION.md)
11. [Pinned ZeroClaw source](https://github.com/zeroclaw-labs/zeroclaw/tree/e112ce6b5ccdac9e1cb166bab217e730dd7e24c2)
12. [Superteam ZeroClaw listing](https://superteam.fun/earn/listing/zeroclaw)

The public Devnet RPC is `https://api.devnet.solana.com` and is rate-limited. The
official faucet currently allows at most two anonymous requests per eight hours; signing
in with GitHub raises its displayed quota. Use the web faucet when CLI airdrops are
rejected. Phantom's documented path is **Settings → Developer Settings → Testnet Mode**,
then select Solana Devnet. A visible testnet banner should remain on.

## 4. Environment variables

### 4.1 Shell-only convenience variables

The plugins do **not** read a `.env` file. These variables only make the commands below
copy-pasteable. ZeroClaw injects plugin config from its own config store.

```bash
cd /path/to/proofkiosk        # your checkout

export PROOFKIOSK_ROOT="$PWD"
export ZEROCLAW_REF="$(tail -n 1 "$PROOFKIOSK_ROOT/wit/UPSTREAM_REF")"
export ZEROCLAW_SRC="$PROOFKIOSK_ROOT/.build/zeroclaw-e112ce6b5ccd"
export ZC_CONFIG_DIR="$PROOFKIOSK_ROOT/.devnet/zeroclaw-config"
export ZC_AGENT="proofkiosk"

export RPC_URL="https://api.devnet.solana.com"
export MERCHANT_KEYPAIR="$PROOFKIOSK_ROOT/.devnet/merchant.json"
export CUSTOMER_KEYPAIR="$PROOFKIOSK_ROOT/.devnet/customer.json"
export ITEM_ID="cold_drink"

mkdir -p "$PROOFKIOSK_ROOT/.devnet" "$ZC_CONFIG_DIR"

# Always routes ZeroClaw commands to this isolated, gitignored test config.
zc() { zeroclaw --config-dir "$ZC_CONFIG_DIR" "$@"; }
```

Variables populated later:

```bash
export MERCHANT=""
export MINT=""
export CUSTOMER=""
export CUSTOMER_ATA=""
export PAY_URL=""
export REFERENCE=""
export PAYMENT_SIG=""
export NONCE_AUTHORITY_KEYPAIR="$MERCHANT_KEYPAIR"
export NONCE_ACCOUNT_KEYPAIR="$PROOFKIOSK_ROOT/.devnet/nonce-account.json"
export NONCE_AUTHORITY=""
export NONCE_ACCOUNT=""
```

Do not put wallet seed phrases, private keys, model API keys, Telegram tokens, or an
authenticated RPC URL in Git, chat, screenshots, or this document.

### 4.2 Environment variables actually recognized by repository scripts

| Variable | Default | Used by |
|---|---|---|
| `MODE` | `localnet` | `scripts/devnet-setup.sh`; use localnet first, then `devnet` for public evidence. |
| `MINT_AMOUNT` | `1000` | Amount of fake tokens initially minted to the separate customer. |
| `PRICE_LIST` | `cold_drink:1.5, snack:0.75` | Catalog emitted into both plugin config rows. |
| `PAYMENT_ITEM` | `cold_drink` | Catalog row selected for `.devnet/payment.env`. |
| `PROOFKIOSK_FINALIZE_TIMEOUT_MS` | `120000` | Optional wait bound used by `scripts/devnet-pay.mjs`. |
| `ZEROCLAW_FEATURES` | `plugins-wasm-cranelift` | Feature list for `install-pinned-zeroclaw.sh`; add `hardware,peripheral-rpi` only for a future Pi build. |
| `ZEROCLAW_BIN` | `.build/zeroclaw-install/bin/zeroclaw` | Optional exact binary override for `host-smoke.sh`. |
| `ZEROCLAW_SOURCE_DIR` | Exact checkout under `.build/` | Exact pinned source used by `exact-host-runtime-smoke.sh`. |
| `PROOFKIOSK_SKIP_STAGE` | `0` | Set internally by `host-smoke.sh` after it has already staged components. |
| `PROOFKIOSK_HOST_OUTPUT` | unset | Fresh path where the exact-host test writes the raw charge WIT `ToolResult`; existing files are refused. |
| `PROOFKIOSK_WATCH_OUTPUT` | unset | Fresh path where the exact-host test writes the raw paid-watch WIT `ToolResult`; `host-smoke.sh` feeds it into the trusted claim. |
| `PROOFKIOSK_ORDERS_DIR` | `.proofkiosk/orders` | Optional trusted QR/order directory override. |
| `CARGO_TARGET_DIR` | `.build/zeroclaw-target` for pinned-host build/runtime; `/tmp/kiosk-verify` or `/tmp/kiosk-size` for artifact gates | Keeps generated Cargo artifacts outside the pristine ZeroClaw checkout; optional override must also stay outside that source tree. |
| `CHROME_PATH` | Auto-detected | Media documentation scripts only. |
| `FFMPEG_PATH` | Auto-detected | Concept-video renderer only. |
| `FFPROBE_PATH` | Auto-detected | Concept-video renderer only. |
| `SAY_PATH` | `/usr/bin/say` | Concept-video narration; macOS syntax required. |
| `PLAYWRIGHT_PATH` | `playwright` | PDF/video renderers only. |
| `RUST_LOG` | Command-dependent | ZeroClaw logging; prefer `--log-level trace -v` for a test run. |

`scripts/devnet-setup.sh` writes `PROOFKIOSK_RPC_URL`,
`PROOFKIOSK_CUSTOMER_KEYPAIR`, `PROOFKIOSK_MERCHANT`, `PROOFKIOSK_MINT`,
`PROOFKIOSK_REFERENCE`, `PROOFKIOSK_ITEM`, `PROOFKIOSK_ITEM_ID`,
`PROOFKIOSK_AMOUNT`, `PROOFKIOSK_MAX_AMOUNT`, `PROOFKIOSK_TOKEN_DECIMALS`,
`PROOFKIOSK_PRICE_LIST`, `PROOFKIOSK_NONCE_ACCOUNT`, and `PROOFKIOSK_CONFIG` into the
gitignored `.devnet/payment.env`. The config path points
to the generated and cross-validated `.devnet/zeroclaw.toml`. Source that generated file;
do not hand-maintain a second copy.

Provider credentials are configured by `zeroclaw quickstart` or `zeroclaw auth`, not by
ProofKiosk. An RPC provider URL containing an API key is a secret even though the public
Devnet URL is not.

## 5. Repair/install prerequisites

### 5.1 Verify Rust; repair only if the check fails

Rust is healthy in the audited workstation state. Run the checks first:

```bash
rustc --version
cargo --version
rustup target list --installed
```

Only if those fail, repair the toolchain:

```bash
rustup update stable
rustup component add rustc cargo clippy rustfmt \
  --toolchain stable-aarch64-apple-darwin
rustup target add wasm32-wasip2 \
  --toolchain stable-aarch64-apple-darwin

rustc --version
cargo --version
rustup target list --installed
```

If `rustc` still says the component is not applicable, repair the toolchain in place:

```bash
rustup toolchain install stable \
  --profile default \
  --component rustc,cargo,clippy,rustfmt \
  --target wasm32-wasip2 \
  --force

rustup default stable
rustc --version
```

### 5.2 Check Solana tools

```bash
solana --version
solana-keygen --version
solana-test-validator --version
spl-token --version
jq --version
node --version
```

Install only what is missing on macOS:

```bash
brew install jq qrencode
```

If the Solana CLI itself is missing, use the current command from the official Solana
installation page:

```bash
curl --proto '=https' --tlsv1.2 -sSfL \
  https://solana-install.solana.workers.dev | bash
```

If only `spl-token` is missing:

```bash
cargo install spl-token-cli --locked
```

## 6. Run every repository build/test gate

There is no root Cargo workspace. Always use `--manifest-path` or change into each
crate.

### 6.1 Host tests

```bash
cd "$PROOFKIOSK_ROOT"

for manifest in \
  crates/kiosk-core/Cargo.toml \
  plugins/kiosk-charge/Cargo.toml \
  plugins/kiosk-watch/Cargo.toml \
  plugins/kiosk-attest/Cargo.toml
do
  cargo test --manifest-path "$manifest" --locked
done
```

Expected Rust total: 213 tests. Then run the 12 trusted-boundary tests:

```bash
npm ci --ignore-scripts --no-audit --no-fund
npm run test:handoff
```

Expected repository total: 225 tests. Rust RPC responses are mocked; passing these tests
does not prove a public Devnet or successful RPC-backed ZeroClaw invocation. The exact
host test in section 7 is counted separately.

### 6.2 Formatting and lint

```bash
for manifest in \
  crates/kiosk-core/Cargo.toml \
  plugins/kiosk-charge/Cargo.toml \
  plugins/kiosk-watch/Cargo.toml \
  plugins/kiosk-attest/Cargo.toml
do
  cargo fmt --manifest-path "$manifest" -- --check
  cargo clippy --manifest-path "$manifest" --all-targets -- -D warnings
done
```

### 6.3 Targeted safety tests

```bash
cargo test --manifest-path plugins/kiosk-charge/Cargo.toml injection_
cargo test --manifest-path plugins/kiosk-watch/Cargo.toml wrong_
cargo test --manifest-path plugins/kiosk-watch/Cargo.toml never_paid
cargo test --manifest-path plugins/kiosk-attest/Cargo.toml rejected
cargo test --manifest-path plugins/kiosk-attest/Cargo.toml \
  tx_contains_only_memo_and_system_programs
cargo test --manifest-path plugins/kiosk-attest/Cargo.toml \
  output_is_unsigned_zero_signatures
```

### 6.4 WASM build, staging, and static checks

```bash
rustup target add wasm32-wasip2
./scripts/stage-plugin.sh

find staged -type f -print -exec wc -c {} \;
./scripts/verify-no-network.sh
./scripts/wasm-size.sh
```

Expected staged directories:

```text
staged/kiosk-charge/
staged/kiosk-watch/
staged/kiosk-attest/
```

`wasm-size.sh` enforces per-component ceilings. `verify-no-network.sh` checks the built
component imports and must show zero `wasi:http` imports for `kiosk-charge`.

## 7. Build the pinned plugin-capable ZeroClaw host

Use the repository installer. It checks out the exact WIT revision vendored here, verifies
the commit, installs with `--locked` and `plugins-wasm-cranelift`, and proves the resulting
binary exposes the plugin command. The pin identifies as ZeroClaw `0.8.2`; it is
intentionally different from the pluginless stock `0.8.3` binary.

```bash
cd "$PROOFKIOSK_ROOT"
./scripts/install-pinned-zeroclaw.sh
export PATH="$PROOFKIOSK_ROOT/.build/zeroclaw-install/bin:$PATH"
```

Then verify the installed binary and all three plugin packages against the isolated host:

```bash

hash -r
zeroclaw --version
zeroclaw plugin --help
./scripts/host-smoke.sh
```

`host-smoke.sh` installs and loads all components, validates the canonical config/SOP
surface, and transitively runs the exact pinned-host runtime test. Deterministic local
JSON-RPC fixtures exercise valid business paths for charge (`created`), watch (`paid`
after two RPC calls), and attest (`signature_required` from a valid nonce/init history
with `minContextSlot`). It also rejects config spoofing, writes real host-direct charge
and paid-watch `ToolResult`s, persists immutable quote terms, creates one exclusive claim,
and proves a duplicate claim fails in an isolated order directory.
This is exact-host local-fixture proof, not public-Devnet evidence.

For a future Raspberry Pi build, the intended feature set is:

```bash
ZEROCLAW_FEATURES=plugins-wasm-cranelift,hardware,peripheral-rpi \
  ./scripts/install-pinned-zeroclaw.sh
```

That Pi build still does not create ProofKiosk's missing `relay_pulse` or `bme280_read`
tools.

## 8. Initialize an isolated ZeroClaw test agent

```bash
cd "$PROOFKIOSK_ROOT"
zc() { zeroclaw --config-dir "$ZC_CONFIG_DIR" "$@"; }

zc quickstart --agent "$ZC_AGENT"
zc agents list
zc auth status
zc self-test --quick
zc security status --agent "$ZC_AGENT"
```

`quickstart` is interactive; select the provider/model you already have permission to
use and enter its credential through the masked prompt. Do not place the model key in a
repository `.env` file.

## 9. Install ProofKiosk plugins into the isolated host

```bash
cd "$PROOFKIOSK_ROOT"

zc plugin install "$PROOFKIOSK_ROOT/staged/kiosk-charge"
zc plugin install "$PROOFKIOSK_ROOT/staged/kiosk-watch"
zc plugin install "$PROOFKIOSK_ROOT/staged/kiosk-attest"

zc config set plugins.enabled true
zc config set plugins.auto_discover true

zc plugin list --all
zc plugin info kiosk-charge
zc plugin info kiosk-watch
zc plugin info kiosk-attest
zc config list --filter plugins.entries
```

The exact pinned source build seeds one empty `plugins.entries` record during each
successful `plugin install`. The last command must show all three plugin names. If it
does not, stop: the config may have a malformed `[plugins]` block, and later `config set`
commands will fail with `Unknown property`. Do not add `[plugins.entries]` by hand; the
actual TOML shape is an array and a syntax mistake can disable the whole plugin section.

Per-plugin values live under canonical `[[plugins.entries]]` rows followed by
`[plugins.entries.config]`. The setup helper prints the exact paste-ready shape.

## 10. Create the complete test environment

The helper creates separate merchant and customer wallets, funds both with test SOL,
creates both token accounts, mints the fake token **only to the customer**, creates a
durable nonce account, generates a fresh Solana Pay reference, writes and validates
`.devnet/zeroclaw.toml`, and writes `.devnet/payment.env` pointing to that config. It
passes `--url` on every CLI call and does not mutate the global Solana CLI configuration.

Start with localnet for a deterministic, zero-value proof; switch `MODE=devnet` only
after this succeeds. `pipefail` prevents `tee` from hiding a failed setup:

```bash
cd "$PROOFKIOSK_ROOT"

(
  set -o pipefail
  MODE=localnet MINT_AMOUNT=1000 ./scripts/devnet-setup.sh \
    | tee "$PROOFKIOSK_ROOT/.devnet/setup-localnet.txt"
)
```

If that command exits nonzero, **stop**; do not source partial state. Confirm the global
CLI URL was not changed, then load the generated handoff:

```bash
solana config get
source "$PROOFKIOSK_ROOT/.devnet/payment.env"

export RPC_URL="$PROOFKIOSK_RPC_URL"
export MERCHANT="$PROOFKIOSK_MERCHANT"
export MINT="$PROOFKIOSK_MINT"
export REFERENCE="$PROOFKIOSK_REFERENCE"
export ITEM_ID="$PROOFKIOSK_ITEM"
export TOKEN_DECIMALS="$PROOFKIOSK_TOKEN_DECIMALS"
export PRICE_LIST="$PROOFKIOSK_PRICE_LIST"
export NONCE_ACCOUNT="$PROOFKIOSK_NONCE_ACCOUNT"
export CUSTOMER="$(solana-keygen pubkey "$PROOFKIOSK_CUSTOMER_KEYPAIR")"

./scripts/check-config.sh "$PROOFKIOSK_CONFIG"

solana cluster-version --url "$RPC_URL"
solana balance "$MERCHANT" --url "$RPC_URL"
solana balance "$CUSTOMER" --url "$RPC_URL"
spl-token balance "$MINT" --owner "$MERCHANT" --url "$RPC_URL"   # expected 0
spl-token balance "$MINT" --owner "$CUSTOMER" --url "$RPC_URL"   # expected 1000
spl-token supply "$MINT" --url "$RPC_URL"
```

If a later balance call is rate-limited, verify the same address in Explorer and retry
after the RPC quota recovers; do not interpret an RPC `429` as a payment result.

## 11. Optional: use a separate Phantom Devnet customer

The primary automated proof uses `.devnet/customer.json`, a throwaway customer distinct
from the merchant. For the video, you may instead use a throwaway Phantom wallet, never a
real wallet. In Phantom:

1. Open **Settings → Developer Settings → Testnet Mode**.
2. Enable testnets and select **Solana Devnet**.
3. Confirm the testnet banner is visible.
4. Copy that Devnet wallet's public address; do not export its seed/private key.

Back in the terminal:

```bash
export CUSTOMER='PASTE_PHANTOM_DEVNET_PUBLIC_ADDRESS_HERE'

solana airdrop 1 "$CUSTOMER" --url "$RPC_URL" || true
solana balance "$CUSTOMER" --url "$RPC_URL"
```

If the CLI airdrop is rate-limited, fund `CUSTOMER` at the official faucet. The customer
needs Devnet SOL for transaction fees.

Create the customer's associated token account and mint fake tokens to it:

```bash
export CUSTOMER_ATA="$(spl-token address \
  --token "$MINT" \
  --owner "$CUSTOMER" \
  --url "$RPC_URL")"

if ! solana account "$CUSTOMER_ATA" --url "$RPC_URL" >/dev/null 2>&1; then
  spl-token create-account "$MINT" \
    --owner "$CUSTOMER" \
    --fee-payer "$MERCHANT_KEYPAIR" \
    --url "$RPC_URL"
fi

spl-token mint "$MINT" 20 "$CUSTOMER_ATA" \
  --mint-authority "$MERCHANT_KEYPAIR" \
  --fee-payer "$MERCHANT_KEYPAIR" \
  --url "$RPC_URL"

spl-token balance "$MINT" --owner "$CUSTOMER" --url "$RPC_URL"
```

Expected customer balance: at least `20` of the fake token. A plain
`spl-token transfer` is **not** a substitute for paying the generated Solana Pay URL,
because it does not include the unique reference pubkey required by `kiosk_watch`.

## 12. Configure all three plugins

Charge configuration:

```bash
zc config set plugins.entries.kiosk-charge.config.merchant_address "$MERCHANT"
zc config set plugins.entries.kiosk-charge.config.usdc_mint "$MINT"
zc config set plugins.entries.kiosk-charge.config.token_decimals "$TOKEN_DECIMALS"
zc config set plugins.entries.kiosk-charge.config.price_list \
  "$PRICE_LIST"
zc config set plugins.entries.kiosk-charge.config.max_amount_usdc '10'
zc config set plugins.entries.kiosk-charge.config.label 'ProofKiosk Devnet'

# Optional cosmetic test label only; these fields never affect the on-chain amount.
# zc config set plugins.entries.kiosk-charge.config.display_currency 'TEST'
# zc config set plugins.entries.kiosk-charge.config.display_rate '1'
```

Watcher configuration:

```bash
export NONCE_ACCOUNT="$(solana-keygen pubkey "$NONCE_ACCOUNT_KEYPAIR")"
export NONCE_AUTHORITY="$MERCHANT"

zc config set plugins.entries.kiosk-watch.config.rpc_url "$RPC_URL"
zc config set plugins.entries.kiosk-watch.config.merchant_address "$MERCHANT"
zc config set plugins.entries.kiosk-watch.config.usdc_mint "$MINT"
zc config set plugins.entries.kiosk-watch.config.token_decimals "$TOKEN_DECIMALS"
zc config set plugins.entries.kiosk-watch.config.price_list \
  "$PRICE_LIST"
zc config set plugins.entries.kiosk-watch.config.device_authority "$NONCE_AUTHORITY"
zc config set plugins.entries.kiosk-watch.config.device_address "$NONCE_ACCOUNT"
zc config set plugins.entries.kiosk-watch.config.device_id 'kiosk-01'
zc config set plugins.entries.kiosk-watch.config.payment_window_s '900'
zc config set plugins.entries.kiosk-watch.config.heartbeat_max_silence_s '1800'
zc config set plugins.entries.kiosk-watch.config.finality 'finalized'
```

Attestation configuration (the nonce account already exists):

```bash
zc config set plugins.entries.kiosk-attest.config.rpc_url "$RPC_URL"
zc config set plugins.entries.kiosk-attest.config.device_id 'kiosk-01'
zc config set plugins.entries.kiosk-attest.config.nonce_account "$NONCE_ACCOUNT"
zc config set plugins.entries.kiosk-attest.config.nonce_authority "$NONCE_AUTHORITY"
zc config set plugins.entries.kiosk-attest.config.allowed_metrics \
  'temp_c:-40:85,humidity:0:100'
zc config set plugins.entries.kiosk-attest.config.custody_mode 't1'
```

Inspect the active plugin section without sharing it publicly:

```bash
zc config list --filter plugins
zc plugin list --all
```

## 13. Test `kiosk_charge`

The security boundary is the exact raw host result, not text returned by an LLM. Source
the setup handoff, require a fresh output path (the runtime test uses `create_new`), then
execute the valid charge through the exact pinned ZeroClaw `WasmTool`:

```bash
cd "$PROOFKIOSK_ROOT"
source "$PROOFKIOSK_ROOT/.devnet/payment.env"

export CHARGE_TOOL_RESULT="$PROOFKIOSK_ROOT/.devnet/charge-tool-result.json"

if [ -e "$CHARGE_TOOL_RESULT" ]; then
  printf 'Refusing to overwrite prior host result: %s\n' "$CHARGE_TOOL_RESULT" >&2
  exit 1
fi

ZEROCLAW_SOURCE_DIR="$ZEROCLAW_SRC" \
PROOFKIOSK_MERCHANT="$PROOFKIOSK_MERCHANT" \
PROOFKIOSK_MINT="$PROOFKIOSK_MINT" \
PROOFKIOSK_TOKEN_DECIMALS="$PROOFKIOSK_TOKEN_DECIMALS" \
PROOFKIOSK_PRICE_LIST="$PROOFKIOSK_PRICE_LIST" \
PROOFKIOSK_ITEM_ID="$PROOFKIOSK_ITEM_ID" \
PROOFKIOSK_HOST_OUTPUT="$CHARGE_TOOL_RESULT" \
  ./scripts/exact-host-runtime-smoke.sh

test -s "$CHARGE_TOOL_RESULT"
```

The file is an exact WIT `ToolResult` wrapper. Its `output` string contains the versioned
charge object; the trusted handoff unwraps it itself. Do not edit or reconstruct it:

```json
{
  "success": true,
  "output": "{\"v\":1,\"success\":true,\"status\":\"created\",...}",
  "error": null
}
```

Validate the raw result against the generated plaintext operator config, persist its
order record durably, and obtain the already-validated canonical URL:

```bash
export PAY_URL="$(node "$PROOFKIOSK_ROOT/scripts/trusted-charge-handoff.mjs" \
  --input "$CHARGE_TOOL_RESULT" \
  --config "$PROOFKIOSK_CONFIG" \
  --orders-dir "$PROOFKIOSK_ROOT/.proofkiosk/orders" \
  --url-only)"

test -n "$PAY_URL"
```

Extract the trusted reference and carry it into the independent payment harness:

```bash
export REFERENCE="$(node -e \
  'console.log(new URL(process.argv[1]).searchParams.get("reference") || "")' \
  "$PAY_URL")"
export PROOFKIOSK_REFERENCE="$REFERENCE"

test -n "$REFERENCE"
test -f "$PROOFKIOSK_ROOT/.proofkiosk/orders/$REFERENCE.json"
printf 'REFERENCE=%s\n' "$REFERENCE"
```

Render the QR only from that same raw host result and config. The renderer repeats the
trusted validation; it never accepts an arbitrary `solana:` URL:

```bash
"$PROOFKIOSK_ROOT/skills/kiosk-qr/render-qr.sh" \
  "$CHARGE_TOOL_RESULT" \
  "$PROOFKIOSK_CONFIG" \
  "$PROOFKIOSK_ROOT/.devnet/payment.png"

open "$PROOFKIOSK_ROOT/.devnet/payment.png"       # macOS
# xdg-open "$PROOFKIOSK_ROOT/.devnet/payment.png" # Linux
```

You may separately ask `zc agent` to call `kiosk_charge` for presentation evidence, but
never feed the agent transcript or model-rendered JSON into the QR, order, watch, claim,
or actuator boundary.

## 14. Check `kiosk_watch` before payment

This agent call is useful presentation/debug evidence, not a trusted actuation input: the
model can reformat tool output. Capture the host-direct WIT result from your external
driver before using the claim helper in a real integration.

```bash
zc agent \
  --agent "$ZC_AGENT" \
  --temperature 0 \
  --message "Call kiosk_watch exactly once with {\"reference\":\"$REFERENCE\",\"item_id\":\"$ITEM_ID\"}. Return the complete raw tool output unchanged." \
  | tee "$PROOFKIOSK_ROOT/.devnet/watch-before.txt"
```

Expected: `PENDING` with `success=false`. If it returns `MISMATCH`, another transaction
already touched that public reference; generate a fresh charge.

Record the merchant balance before payment:

```bash
export MERCHANT_BEFORE="$(spl-token balance "$MINT" \
  --owner "$MERCHANT" --url "$RPC_URL")"
printf 'merchant_before=%s\n' "$MERCHANT_BEFORE"
```

## 15. Pay with the finalized Solana Pay harness

The primary path uses the throwaway customer keypair, never the merchant key. The script
creates a reference-bearing SPL transfer with the official Solana Pay package, preserves
the charge's `PKPAY1` reference/item memo, submits it, waits for `finalized`, and calls
`validateTransfer` over recipient, mint, amount, and reference:

```bash
cd "$PROOFKIOSK_ROOT"
npm ci

(
  set -o pipefail
  npm run devnet:pay \
    | tee "$PROOFKIOSK_ROOT/.devnet/payment-submit.txt"
)

export PAYMENT_SIG="$(awk '/^submitted / {print $2; exit}' \
  "$PROOFKIOSK_ROOT/.devnet/payment-submit.txt")"
test -n "$PAYMENT_SIG"
grep -F "finalized and validated $PAYMENT_SIG" \
  "$PROOFKIOSK_ROOT/.devnet/payment-submit.txt"
```

The known-good localnet proof moved `1.5` test tokens from the separate customer to the
merchant (merchant `0 → 1.5`, customer `1000 → 998.5`) and reached finalized commitment.
This is an independent Solana Pay transfer proof. It does **not** invoke `kiosk_watch`;
the plugin verifier has its own mocked adversarial suite, and a valid host-direct
RPC-backed invocation remains separate evidence.

### Optional browser-wallet path

On the Phantom mobile wallet configured for Solana Devnet:

1. Scan `.devnet/payment.png` or open the `solana:` link.
2. Confirm the wallet still shows the testnet/Devnet banner.
3. Verify the recipient equals `MERCHANT`.
4. Verify the mint equals `MINT`; it may be displayed as an unknown custom token.
5. Verify the amount equals `1.5`.
6. Approve using the throwaway customer wallet.
7. Copy the transaction signature into `PAYMENT_SIG` instead of using the harness value.

Never approve if the wallet switched to Mainnet, displays real USDC, or shows different
recipient/mint/amount values.

## 16. Inspect the payment independently, then re-run the watcher

Query by reference using raw RPC:

```bash
curl -sS "$RPC_URL" \
  -H 'content-type: application/json' \
  --data "$(jq -nc --arg ref "$REFERENCE" \
    '{jsonrpc:"2.0",id:1,method:"getSignaturesForAddress",params:[$ref,{commitment:"finalized",limit:10}]}')" \
  | tee "$PROOFKIOSK_ROOT/.devnet/reference-signatures.json" \
  | jq .

# Keep the real signature already captured in section 15. For the optional Phantom path,
# set PAYMENT_SIG from the wallet before reaching this point. Never overwrite it with a
# placeholder or silently choose "newest".
: "${PAYMENT_SIG:?Set PAYMENT_SIG from the finalized harness or Phantom receipt}"

jq -e --arg sig "$PAYMENT_SIG" \
  '.result | any(.signature == $sig)' \
  "$PROOFKIOSK_ROOT/.devnet/reference-signatures.json"

solana confirm "$PAYMENT_SIG" \
  --commitment finalized \
  --url "$RPC_URL" \
  --verbose
```

Fetch the parsed transaction:

```bash
curl -sS "$RPC_URL" \
  -H 'content-type: application/json' \
  --data "$(jq -nc --arg sig "$PAYMENT_SIG" \
    '{jsonrpc:"2.0",id:1,method:"getTransaction",params:[$sig,{encoding:"jsonParsed",commitment:"finalized",maxSupportedTransactionVersion:0}]}')" \
  | tee "$PROOFKIOSK_ROOT/.devnet/payment-transaction.json" \
  | jq '{slot:.result.slot,err:.result.meta.err,pre:.result.meta.preTokenBalances,post:.result.meta.postTokenBalances}'
```

Explorer URLs:

```bash
printf 'Transaction: https://explorer.solana.com/tx/%s?cluster=devnet\n' \
  "$PAYMENT_SIG"
printf 'Reference:   https://explorer.solana.com/address/%s?cluster=devnet\n' \
  "$REFERENCE"
printf 'Mint:        https://explorer.solana.com/address/%s?cluster=devnet\n' \
  "$MINT"
```

Check balance movement:

```bash
export MERCHANT_AFTER="$(spl-token balance "$MINT" \
  --owner "$MERCHANT" --url "$RPC_URL")"
export CUSTOMER_AFTER="$(spl-token balance "$MINT" \
  --owner "$CUSTOMER" --url "$RPC_URL")"

printf 'merchant_before=%s\nmerchant_after=%s\ncustomer_after=%s\n' \
  "$MERCHANT_BEFORE" "$MERCHANT_AFTER" "$CUSTOMER_AFTER"
```

Now call the watcher:

```bash
zc agent \
  --agent "$ZC_AGENT" \
  --temperature 0 \
  --message "Call kiosk_watch exactly once with {\"reference\":\"$REFERENCE\",\"item_id\":\"$ITEM_ID\"}. Return the complete raw tool output unchanged." \
  | tee "$PROOFKIOSK_ROOT/.devnet/watch-after.txt"
```

Expected business output: `status:"paid"` with the same `PAYMENT_SIG`,
the immutable `amount`, `recipient`, `mint`, `token_decimals`, `payment_window_s`, and
`payment_block_time_s`, plus `payment_verified:true`, `actuation_authorized:false`, and
`requires_atomic_claim:true`. Capture the tool trace:

```bash
zc doctor traces --contains kiosk_watch --limit 20
zc doctor traces --contains kiosk_charge --limit 20
```

This proves a real finalized test-token payment case, not safe authorization for
hardware. Section 13 created a crash-safe trusted order record, but the `zc agent` text
above is not the raw host-direct watch `ToolResult` required by
`trusted-order-claim.mjs`. Do not synthesize that file from model prose. The amount is
not a tool argument: `kiosk_watch` derives it from operator-owned `price_list`, and the
operator-configured decimals must match the real mint.

## 17. Test `kiosk_attest` only up to unsigned-message creation

The setup helper already created `.devnet/nonce-account.json` with the throwaway merchant
as nonce authority. A fresh account now bootstraps correctly: chain recovery recognizes
the real successful System-program nonce initialization, while unrelated public history
and outsider memos do not become an authenticated chain head.

```bash
export NONCE_ACCOUNT="$(solana-keygen pubkey "$NONCE_ACCOUNT_KEYPAIR")"
export NONCE_AUTHORITY="$MERCHANT"

solana nonce "$NONCE_ACCOUNT" --url "$RPC_URL"
```

Build an unsigned reading:

```bash
zc agent \
  --agent "$ZC_AGENT" \
  --temperature 0 \
  --message 'Call kiosk_attest exactly once with {"kind":"reading","metric":"temp_c","value":4.2}. Return the complete raw tool output unchanged.' \
  | tee "$PROOFKIOSK_ROOT/.devnet/attest-output.txt"
```

Expected: one JSON object with `"success":true`, `"status":"signature_required"`, a
summary beginning `BUILT`, and the complete `unsigned_message_base64`. Nothing has been
signed, submitted, or finalized.

Copy the base64 value and inspect the bare message bytes portably:

```bash
export UNSIGNED_MESSAGE_B64='PASTE_UNSIGNED_MESSAGE_BASE64_VALUE_HERE'

node -e \
  'process.stdout.write(Buffer.from(process.argv[1], "base64"))' \
  "$UNSIGNED_MESSAGE_B64" \
  | xxd \
  | head
```

**Stop here.** The repository has no signer firewall, signature-vector assembly,
`sendTransaction`, idempotency lock, or finality confirmation. A generic “sign arbitrary
base64” helper would destroy the intended security boundary and must not be used as the
submission solution.

One nonce account supports one pending artifact. A future driver must serialize build →
validate/sign → submit → finalized before asking `kiosk_attest` to build the next
artifact from that account.

## 18. Authenticated heartbeat test

```bash
zc agent \
  --agent "$ZC_AGENT" \
  --temperature 0 \
  --message 'Call kiosk_watch exactly once with {"mode":"heartbeat"}. Return the complete raw tool output unchanged.'
```

The target address, device id, authority, and `heartbeat_max_silence_s` are operator
config, not tool arguments. The scan ignores a fresh public memo unless the transaction
succeeded, contains the configured device account, carries the configured authority
signer, and contains an exact versioned memo for the configured device id. Output is
structured JSON: `live` has `success:true`; `stale` and `missing` have `success:false`.

A fresh nonce initialization is not itself a heartbeat, so a new deployment should
return `missing` until an authenticated attestation has actually landed.

## 19. Validate the SOP files—but do not run the relay flow

```bash
zc config set sop.sops_dir "$PROOFKIOSK_ROOT/sops"
zc config set sop.step_scope_enforce true

zc sop list
zc sop validate
zc sop validate proofkiosk-payment-loop
zc sop validate proofkiosk-sensor-loop
zc sop validate proofkiosk-heartbeat

zc sop graph proofkiosk-payment-loop --format outline
zc sop graph proofkiosk-sensor-loop --format outline
zc sop graph proofkiosk-heartbeat --format outline
```

Validation proves syntax only. Do not use these SOPs to energize hardware because:

- the checked-in SOP still has literal reference/item placeholders;
- trusted charge persistence and an exclusive host-local claim exist, but no shipped
  driver captures a raw host-direct watch result and connects the claim to hardware;
- watcher output now includes routeable JSON `success`/`status` fields;
- the exact pin dispatches cron, but these ordinary tool/execute steps have no headless
  execution driver; the manifests also omit `sop.agent`, and adding it alone would not
  make the flows runnable;
- `relay_pulse`, `bme280_read`, and `notify_operator` are placeholders;
- the claim is at-most-once on one host, not exactly-once physical delivery; a crash
  after claim but before actuation needs an operator recovery policy.

## 20. Optional Telegram channel test

First complete the local CLI flow. A channel adds presentation evidence, not payment
safety.

If the pinned ZeroClaw build lacks Telegram support, rebuild ZeroClaw from its pinned
checkout with the appropriate channel/gateway features as well as
`plugins-wasm-cranelift`. Then create a bot only through Telegram's verified
[`@BotFather`](https://t.me/BotFather).

At the exact source pin, `channel add` is not the configuration path. Set the secret
property without a value so ZeroClaw opens its masked prompt, then enable and bind the
same non-default alias:

```bash
zc config set channels.telegram.proofkiosk-devnet.bot_token
zc config set channels.telegram.proofkiosk-devnet.enabled true
zc channel list
zc channel bind-telegram YOUR_TELEGRAM_USERNAME \
  --alias proofkiosk-devnet
zc channel doctor
zc daemon --host 127.0.0.1 --log-level trace -v
```

Run the same charge → pay → watcher conversation. Capture the inbound chat, tool
arguments, sanitized config, tool result, Explorer signature, and merchant balance
change. Never show the bot token or wallet secrets.

## 21. Hardware inspection only

These discovery commands require the future Pi/hardware-feature build from section 7;
the primary laptop build will report that hardware support is unavailable:

```bash
zc hardware discover
zc peripheral list
```

On a Raspberry Pi with I²C enabled:

```bash
sudo apt-get update
sudo apt-get install -y i2c-tools
i2cdetect -y 1
```

Expected BME280 address: `0x76` or `0x77`.

Do not run GPIO against a connected motor/solenoid. The repository does not implement
the named relay/sensor tools, does not define active-high/active-low polarity, has no
hardware one-shot, and its wiring guide contains an isolation/common-ground
contradiction. First test a current-limited LED through a reviewed driver design, then
add a fused, fail-safe actuator service outside model control.

## 22. Runtime negative-test checklist

Record the exact prompt, actual tool JSON, tool result, trace, and absence of hardware
action for each case.

| Test | Command/prompt | Correct current expectation |
|---|---|---|
| Unknown item | Call `kiosk_charge` with `{"item_id":"free_everything"}` | Rejected. |
| Over cap | Call charge with `{"amount_usdc":"9999"}` | Rejected. |
| Smuggled recipient | Force exact JSON with an extra `recipient` field | Unknown field rejected. |
| Query injection | Put `&amount=0&recipient=...` in `note` | Encoded as text, not live query fields. |
| Before payment | Valid reference + catalog item | JSON `status:"pending", success:false`. |
| Smuggled expected amount | Add `expected_amount` to watcher args | Unknown field rejected; amount comes from operator `price_list[item_id]`. |
| Underpayment | Pay less than the configured catalog price | `mismatch`, never paid. |
| Equal-priced wrong item | Reuse a payment/reference under another item id | `mismatch`; the `PKPAY1` memo binds the original item. |
| Payment after quote expiry | Persist a one-second quote, then land the payment after `expires_at_ms` | Watch can report the chain fact, but the trusted claim rejects `payment_block_time_s`; a missing block time fails closed. |
| Catalog changed after quote | Persist at 1.5, change both plugin catalogs to 0.01, then verify a 0.01 payment | Watch may verify current config; the trusted claim rejects economics drift against the 1.5 order. |
| RPC unavailable | Temporarily configure an invalid test RPC | Failed WIT `ToolResult`: outer `success:false`, empty `output`, populated `error`; restore config immediately. |
| Unknown metric | Attest `pressure` when not allowlisted | Rejected. |
| Out-of-range reading | Attest `temp_c:999` | Rejected. |
| Repeat watcher before marker | Call the same paid reference twice | The plugin can return `paid` again; the second trusted host-local exclusive claim must fail. Never connect the plugin result alone to an actuator. |
| Authenticated fulfillment marker | Recheck after the configured authority lands `PKFUL1` | `already_fulfilled`, `success:false`. |
| Newer decoys on reference | Add junk/fake markers before a real candidate | All candidates in the ten-entry window are scanned/authenticated; more than ten newer writes remain a bounded-window risk. |
| Fake heartbeat | Fresh unsigned/wrong-signer memo touching the device account | Ignored; returns `missing`/uses the newest authenticated device memo. |
| Non-T1 custody config | Set `custody_mode=t2` | Config rejected; only `t1` is accepted. |

Useful exact host-test commands:

```bash
cargo test --manifest-path plugins/kiosk-charge/Cargo.toml \
  injection_smuggled_recipient_key_is_a_serde_error
cargo test --manifest-path plugins/kiosk-charge/Cargo.toml \
  injection_note_cannot_forge_url_params
cargo test --manifest-path plugins/kiosk-watch/Cargo.toml \
  old_payment_is_reported_for_trusted_quote_time_validation
cargo test --manifest-path plugins/kiosk-watch/Cargo.toml \
  on_chain_failed_tx_is_mismatch_not_paid
cargo test --manifest-path plugins/kiosk-watch/Cargo.toml \
  malformed_get_transaction_is_err_never_paid
cargo test --manifest-path plugins/kiosk-attest/Cargo.toml \
  value_nan_or_inf_rejected
cargo test --manifest-path plugins/kiosk-attest/Cargo.toml \
  tx_contains_only_memo_and_system_programs
```

## 23. Remaining blockers, ranked

### P0—required before a physical production claim

1. **Trusted driver + actuator recovery state machine.** The charge handoff and
   single-host exclusive claim are shipped, but no driver captures the raw host-direct
   paid result, claims the order, transitions claimed → actuating → delivered, enforces a
   fixed pulse/cooldown, and recovers safely after a crash.
2. **Actuator and delivery sensor.** `relay_pulse`, `bme280_read`, boot-safe GPIO policy,
   one-shot hardware, and delivery sensing are not implemented. The wiring document is
   a design guide, not evidence of a build.
3. **Signer/submission firewall.** `kiosk_attest` stops at unsigned message bytes. A
   separate process must decode and constrain the message, sign, submit, wait for
   finalization, and serialize one pending artifact per durable nonce.
4. **Sensor provenance.** Bounds reject implausible model-supplied values but do not prove
   a physical BME280 produced them.

### P1—remaining code/infrastructure risk

1. **Ten-signature crowding.** More than ten newer public writes can push a real payment
   or marker out of the bounded scan. Add a checkpoint/indexer or carefully bounded
   pagination. The local exclusive claim remains mandatory for actuation.
   Attestation recovery has a separate bound: it scans at most 100 public device-address
   writes to prove ten authenticated links, so heavy crowding can halt new attestations
   and can require up to 100 sequential transaction fetches.
2. **RPC deadline and trust.** Envelope checks, non-2xx handling, connect timeout, and a
   two-MiB cap exist, but there is no full post-connect response/read deadline. The
   operator RPC remains a trust root; quorum is not implemented.
3. **Generic HTTP capability.** T0 means no fund custody, not no network reach. The exact
   host does not limit watch/attest to one RPC origin/method.
4. **Public-Devnet host-direct evidence.** Exact-host CI now exercises valid local-fixture
   business paths for all three components. A public-Devnet raw host-direct watch/attest
   capture is still absent and would strengthen the submission evidence.
5. **Unsigned plugin packages.** Builds are pinned and hashed, but release artifact
   publisher signatures/verification are not shipped.

### P2—evidence polish

Record a clean commit, redacted generated config, exact host output, Explorer/localnet
transaction evidence, raw host-direct paid result, exclusive claim, actuator/sensor
trace, finalized signed attestation, and negative cases. Do not substitute the independent
`devnet-pay.mjs` transfer validation for `kiosk_watch` execution evidence.

## 24. Evidence bundle for a credible final submission

You can record an honest payment-rail video now. Do not label it a full autonomous
hardware/attestation end-to-end run until every P0 item above is fixed and evidenced.
The evidence directory should contain:

```text
evidence/
  commit.txt
  tool-versions.txt
  redacted-config.toml
  host-tests.txt
  wasm-hashes.txt
  channel-transcript.txt
  charge-output.json
  watch-before.json
  payment-signature.txt
  payment-transaction.json
  watch-after.json
  order-state-transitions.jsonl
  actuator-audit.jsonl
  gpio-logic-trace.png
  delivery-sensor-event.json
  attestation-signature.txt
  attack-transcript.txt
  continuous-demo.mp4
```

Capture commands after the implementation is fixed:

```bash
mkdir -p evidence
git rev-parse HEAD | tee evidence/commit.txt

{
  rustc --version
  cargo --version
  zeroclaw --version
  solana --version
  spl-token --version
} | tee evidence/tool-versions.txt

shasum -a 256 staged/*/*.wasm | tee evidence/wasm-hashes.txt
```

The final video must show one continuous real flow: customer channel → charge → wallet
review → Devnet signature → trusted order verification → exactly one safe actuator pulse
→ physical delivery detection → signed/finalized attestation → replay and injection
rejection.

## 25. Troubleshooting commands

```bash
# Solana
solana config get
solana cluster-version --url "$RPC_URL"
solana balance "$MERCHANT" --url "$RPC_URL"
solana balance "$CUSTOMER" --url "$RPC_URL"
spl-token accounts --owner "$MERCHANT" --url "$RPC_URL"
spl-token accounts --owner "$CUSTOMER" --url "$RPC_URL"
solana transaction-history "$REFERENCE" --limit 10 --url "$RPC_URL"

# ZeroClaw
zc status
zc self-test --quick
zc plugin list --all
zc plugin info kiosk-charge
zc plugin info kiosk-watch
zc plugin info kiosk-attest
zc config list --filter plugins
zc sop list
zc sop validate
zc doctor traces --limit 50

# Verbose one-shot agent call
zc --log-level trace -v agent \
  --agent "$ZC_AGENT" \
  --message 'List the ProofKiosk tools available to you. Do not call them.'
```

Public Devnet RPC `429` responses mean rate limiting, not payment failure. Wait, reduce
polling, or use an operator-controlled Devnet RPC endpoint. Never allow a model to choose
or override that endpoint.

## 26. End-of-session safety

```bash
unset PAY_URL REFERENCE PAYMENT_SIG UNSIGNED_MESSAGE_B64
unset CUSTOMER CUSTOMER_ATA MERCHANT MINT
unset NONCE_AUTHORITY NONCE_ACCOUNT
unset CHARGE_TOOL_RESULT TOKEN_DECIMALS PRICE_LIST
unset PROOFKIOSK_REFERENCE PROOFKIOSK_HOST_OUTPUT PROOFKIOSK_ORDERS_DIR
```

If a local validator was used, inspect and stop only its recorded PID:

```bash
if [ -f "$PROOFKIOSK_ROOT/.devnet/validator.pid" ]; then
  validator_pid="$(cat "$PROOFKIOSK_ROOT/.devnet/validator.pid")"
  validator_command="$(ps -p "$validator_pid" -o command= 2>/dev/null || true)"

  case "$validator_pid" in
    ''|*[!0-9]* )
      printf 'Refusing invalid/stale validator PID record: %s\n' \
        "$validator_pid" >&2
      ;;
    * )
      case "$validator_command" in
        *solana-test-validator*"$PROOFKIOSK_ROOT/.devnet/ledger"* )
          printf 'Stopping verified local validator: %s\n' "$validator_command"
          kill "$validator_pid"
          rm "$PROOFKIOSK_ROOT/.devnet/validator.pid"
          ;;
        * )
          printf 'Refusing to signal PID %s; command did not match the expected validator: %s\n' \
            "$validator_pid" "$validator_command" >&2
          ;;
      esac
      ;;
  esac
fi
```

Keep `.devnet/` private. Its keypairs are test-only but still usable signing keys on
Devnet. Never reuse them on Mainnet or for anything valuable.
