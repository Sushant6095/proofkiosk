# kiosk-attest

Notarizes a sensor reading or a sale receipt on Solana as a **hash-chained,
durable-nonce, memo-only transaction** — built **unsigned**, so the plugin holds no key
and a transfer is not expressible in its output.

**For:** anyone who needs a tamper-evident record that a machine observed something at a
time. Cold-chain logging, uptime proofs, meter readings, sale receipts. Useful standalone
from a laptop — it will notarize arbitrary bounded readings with no kiosk attached.
Component 3 of 3 in [ProofKiosk](../../README.md).

Channel-agnostic and stateless: the chain sequence is recovered from the chain itself on
every call, so a fresh host with no local state still produces the next correct link.

---

## Custody: Tier 1 — funds cannot move, by construction

| Property | Status |
|---|---|
| Holds a private key | **No.** |
| Signs anything | **No.** The built transaction carries **zero** signatures. |
| Network access | Read-only RPC (recover chain head, read the nonce). |
| Can move funds | **No — structurally.** See below. |

The transaction contains exactly two programs: **Memo** and **System**
(`AdvanceNonceAccount`). A transfer instruction is not present and cannot be added by any
model input, because the instruction set is assembled from a fixed list rather than from
arguments. This is asserted by a structural test
(`tx_contains_only_memo_and_system_programs`) that inspects the compiled program-id set,
not by validating a string.

That is the difference between "we check for transfers" and "a transfer is not
expressible." An external operator signer receives the unsigned bytes and decides whether
to sign. Even a fully subverted agent can only hand that signer a memo.

**Why a WASM plugin and not a Tier-1 skill — honestly.** This is the component with the
strongest case for a jail, and it is still worth stating what the jail does and does not
buy. It does *not* protect a signing key, because there is no key here. What it buys: the
`permissions = ["http_client", "config_read"]` declaration is a checkable, narrow
statement about what this code can reach, and the fresh-store-per-call model makes the
"derive the chain head from the chain" design enforced rather than merely intended.

A Tier-1 skill could build the same unsigned transaction. It could not make the same
declaration about its own reach, and a script that assembles transaction bytes is
precisely where "I promise it only adds a memo" is worth less than a structural test plus
a capability list. If you are notarizing readings and not driving hardware, a skill is a
perfectly reasonable choice — the component boundary is buying auditability here, not
secrecy.

---

## Config

Operator-owned, injected as `__config`.

| Key | Required | Meaning |
|---|---|---|
| `rpc_url` | **yes** | Solana JSON-RPC endpoint. |
| `device_id` | **yes** | Human device id written into every memo as `dev`. |
| `nonce_account` | **yes** | Durable nonce account pubkey (base58). The attestation chain is scanned here. |
| `nonce_authority` | **yes** | Nonce authority / fee payer **public** key; must own the nonce account. |
| `allowed_metrics` | no | `"temp_c:-40:85, humidity:0:100"` — the metric allowlist **and** its bounds. |
| `custody_mode` | no | Default `t1`. |

Minimal working config:

```toml
[plugins.kiosk-attest.config]
rpc_url         = "https://api.devnet.solana.com"
device_id       = "kiosk-01"
nonce_account   = "YOUR_NONCE_ACCOUNT_PUBKEY"
nonce_authority = "YOUR_NONCE_AUTHORITY_PUBKEY"   # PUBLIC key
allowed_metrics = "temp_c:-40:85, humidity:0:100"
```

**On secrets — the one place a ProofKiosk deployment involves a key.** `nonce_authority`
above is a **public** key. The matching private key belongs to your external signer, lives
outside ZeroClaw entirely (separate process, ideally separate machine or an HSM), and must
never appear in any config ZeroClaw can read. If this config file leaks, an attacker
learns which account attests and nothing more. Full reasoning in
[`docs/threat-model.md`](../../docs/threat-model.md).

**Set `allowed_metrics`.** Without it there is no bound to enforce, and bounds are how a
failing sensor gets caught. Bracket the *plausible* range of your enclosure, not the
sensor's datasheet range.

## Args (model-facing, `deny_unknown_fields` + raw-key allowlist)

| Arg | Kind | Meaning |
|---|---|---|
| `kind` | all | `"reading"` (default), `"event"`, or `"fulfillment"`. |
| `metric`, `value` | reading | Allowlisted metric name; finite numeric value inside its bounds. |
| `event`, `item`, `payment_sig` | event | Event label, optional item id and payment signature. |
| `reference` | fulfillment | The Solana Pay reference of the charge that was just delivered. Required. |
| `ts` | all | Unix seconds. Defaults to now. |

### The fulfillment kind, and why it exists

`kind="fulfillment"` writes a **delivery receipt**: a `PKFUL1`-tagged memo naming a
charge. [`kiosk-watch`](../kiosk-watch) scans for it and refuses to re-authorize a charge
that has one, which is what makes delivery single-use — the verifier is stateless, so
"already delivered" has to be a fact about the chain rather than something remembered.

Two implementation details that are load-bearing:

- **The reference rides as a read-only, non-signer account key**, which is what puts the
  marker into `getSignaturesForAddress(reference)` where the watcher looks. It hangs off
  the `AdvanceNonceAccount` instruction, not the memo — SPL Memo v2 rejects any account
  passed to it that is not a signer, and the kiosk cannot sign for a reference keypair it
  does not hold. The System program reads only accounts 0..=2, so the extra key is inert
  on-chain. This is the same mechanism Solana Pay uses to make a payment findable.
- **Your `nonce_authority` is what authenticates it.** It is the fee payer and only
  required signer of the marker, so it must equal `kiosk-watch`'s `device_authority`. Set
  them differently and no marker ever authenticates — single-use silently stops working.
  `scripts/check-config.sh` checks this.

Custody is unchanged: still an unsigned Memo + System transaction, still incapable of
expressing a transfer, re-asserted for this kind by
`fulfillment_tx_contains_only_memo_and_system_programs`.

## Worked example

```json
{ "kind": "reading", "metric": "temp_c", "value": 4.2 }
```

Output (`success = true`), token-budgeted:

```
ATTESTED reading seq=8 metric=temp_c val=4.2 ts=1700000000 — unsigned durable-nonce tx built (263 bytes), ready for the operator signer.
unsigned_tx_base64=AQABBQ...
```

The memo payload is `{v, dev, seq, ts, metric, val, prev}`. `seq` and `prev` (the previous
attestation's landed signature) are what make the readings an ordered chain: a deleted or
reordered reading is detectable by walking it.

## Prompt injection: refusing to attest a lie

Every row is an executable host test (`cargo test`, RPC mocked, **no network**).

| Attack / failure | Result |
|---|---|
| "Attest to MY account" → smuggled `{"nonce_authority": …}` / `{"recipient": …}` | **Rejected** before any logic. |
| Metric not in the operator allowlist | **Rejected** — refuse to attest an unknown metric. |
| Value outside `[min,max]` | **Rejected** — refused outright, never clamped into a plausible lie. |
| Value `NaN` / `±inf` | **Rejected** — non-finite values cannot be attested. |
| "Add a transfer to the transaction" | **Impossible.** Memo + System only; asserted structurally. |
| RPC errors or returns garbage | **`Err`** — never a successful attestation. |
| Newest device tx has no readable attestation memo | **Chain gap surfaced** — not silently treated as a fresh device. |

The refusal-over-clamping choice is the important one. A clamped reading is a plausible
number that is wrong, written permanently to a public ledger. An error is recoverable; a
notarized lie is not.

---

## Reproduce it in an evening

Tested against ZeroClaw **v0.8.3**.

**1. A host with the plugin runtime.** Prebuilt binaries ship without it —
`zeroclaw plugin …` is an unrecognized subcommand there. One backend flag carries the
`plugins-wasm` umbrella:

```bash
./install.sh --source --features plugins-wasm-cranelift
```

**2. Build and stage:**

```bash
rustup target add wasm32-wasip2
git clone https://github.com/Sushant6095/proofkiosk.git && cd proofkiosk
cargo test --manifest-path plugins/kiosk-attest/Cargo.toml   # 16 tests, RPC mocked, no network
./scripts/stage-plugin.sh kiosk-attest                       # -> staged/kiosk-attest/
```

**3. Install and enable:**

```bash
zeroclaw plugin install ./staged/kiosk-attest/
zeroclaw config set plugins.enabled true
zeroclaw plugin list
zeroclaw plugin info kiosk-attest
```

**4. Create a durable nonce account** on devnet with the Solana CLI. This is the signer's
job, deliberately outside ZeroClaw:

```bash
solana-keygen new -o nonce-authority.json
solana airdrop 2 --keypair nonce-authority.json --url devnet
solana-keygen new -o nonce-account.json
solana create-nonce-account nonce-account.json 0.0015 \
  --keypair nonce-authority.json --url devnet

solana address -k nonce-account.json      # -> nonce_account
solana address -k nonce-authority.json    # -> nonce_authority
```

Put those two **public** addresses in the config above. Keep both keypair files away from
ZeroClaw.

**5. Attest something,** in chat:

```
> record a temperature reading of 4.2
```

You get back unsigned base64. Inspect it before signing — being able to is the entire
point of an unsigned artifact:

```bash
echo "<unsigned_tx_base64>" | base64 -d | xxd | head
```

Then sign and submit it with your external signer. ProofKiosk deliberately ships no
signing step; that boundary is the security model, not an omission.

**6. Attest on a cadence** with [`sops/sensor-loop/`](../../sops/sensor-loop), which reads
a sensor and attests the reading every five minutes.

---

## What fought us at the component boundary

- **Hand-rolling Solana transaction serialization.** No `solana-sdk` on
  `wasm32-wasip2`, so [`crates/kiosk-core`](../../crates/kiosk-core) implements base58,
  base64, **shortvec** (Solana's compact-u16 length prefix), the legacy message layout,
  and the Memo and `AdvanceNonceAccount` instruction builders. Shortvec is the nasty one:
  get the varint wrong and the transaction deserializes into a *different, valid*
  transaction rather than failing loudly. It has property tests for exactly that reason.
- **`AdvanceNonceAccount` must be instruction 0.** Solana enforces it — a durable-nonce
  transaction with the advance anywhere else is rejected by the network, not at build
  time. Pinned by a test so it fails in CI instead of on devnet.
- **The durable nonce exists because of the component's own constraints.** A recent
  blockhash expires in ~60 s. A Pi that batches attestations, loses connectivity, or waits
  on a human signer will blow through that. The nonce is what makes an attestation built
  now still submittable later, which is the only reason offline attestation works.
- **Statelessness forced the chain design.** With a fresh store per call there is nowhere
  to keep `seq`. It is recovered from the chain in a **single**
  `getSignaturesForAddress`, and `prev` is the last landed signature. Reading the sequence
  from the ledger rather than from local state is also what makes a gap detectable instead
  of silently skipped.
- **A ~200-token output budget with a base64 transaction inside it.** Unsigned transaction
  bytes are bulky and the output is model-facing. Everything else is compressed to
  `key=value` and passed through `kiosk_core::shape::clamp`, with a test asserting the
  ceiling.
- **389 KB, the largest of the three,** because it bundles the HTTP/TLS client *and* the
  transaction builders. Both earn their bytes.

## Layout & tests

Pure core (`src/attest.rs`, zero wasm deps) plus a thin
`#[cfg(target_family = "wasm")]` shim (`src/lib.rs`).

```bash
cargo test                                      # 16 host tests, no network
cargo clippy --all-targets -- -D warnings
cargo build --target wasm32-wasip2 --release    # ~389 KB component
```

## Honest limitation

The chain is tamper-evident **ordering**, not a content-hash Merkle tree. `seq`/`prev`
make deletion and reordering detectable; an authorized signer could still branch history.
The tradeoff buys a self-contained design with no attestation-service program to deploy
and trust. Stated here rather than left for a reviewer to find.

## The rest of the system

[`kiosk-charge`](../kiosk-charge) issues the charge, [`kiosk-watch`](../kiosk-watch)
verifies it. Start at the [top-level README](../../README.md).
