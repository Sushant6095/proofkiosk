# ProofKiosk — a pay-to-actuate, self-attesting kiosk on Solana

[![ci](https://github.com/Sushant6095/proofkiosk/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Sushant6095/proofkiosk/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

ProofKiosk is a [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) agent that **sells
a physical item for USDC, delivers it only after the payment is verified on-chain, and
writes tamper-evident attestations of what it did** — while the agent never holds a
spendable key.

<!-- ───────────────────────────────────────────────────────────────────────────
     TODO — final submission text goes here.

     - [ ] Demo video (3 min):  <VIDEO_LINK>
     - [ ] Write-up:            <WRITE_UP>

     Placeholder left deliberately; Sushant supplies the final copy.
     ─────────────────────────────────────────────────────────────────────── -->

> **Demo video:** _coming soon_ — `<VIDEO_LINK>`
> **Write-up:** _coming soon_ — `<WRITE_UP>`

---

## The two ideas that make it interesting

1. **The agent never holds a spendable key.** Money flows customer wallet → merchant
   wallet directly; the agent only prints the invoice. Jailbreaking the chatbot yields
   no till to raid — the recipient is fixed by operator config and is unreachable from
   the prompt.
2. **The relay fires on a verified on-chain payment, not on what the agent believes.**
   `kiosk-watch` returns `success == true` **iff** the exact USDC amount reached the
   merchant at the configured finality. The actuation SOP gates on that one boolean.
   Pending, mismatch, expiry, and RPC failure all fail closed.

Both claims are backed by host tests and, where possible, checked against the compiled
binary rather than asserted in prose. See [`docs/threat-model.md`](docs/threat-model.md).

## What's in here

It is a *system*, not a single plugin: three small WIT tool plugins over one shared pure
crate.

| Component | Tier | Question it answers | Network |
|---|---|---|---|
| [`plugins/kiosk-charge`](plugins/kiosk-charge) | T1 | "What should the customer pay?" → a Solana Pay `solana:` URL | **none** |
| [`plugins/kiosk-watch`](plugins/kiosk-watch) | T0 | "Did the money actually arrive?" → verified / pending / mismatch | read-only RPC |
| [`plugins/kiosk-attest`](plugins/kiosk-attest) | T1 | "Prove what happened." → hash-chained, durable-nonce memo tx (unsigned) | read-only RPC |
| [`crates/kiosk-core`](crates/kiosk-core) | — | shared pure substrate: base58/base64, shortvec, Solana Pay, memo/nonce/message builders, JSON-RPC seam, output shaping | — |

Custody tiers (T0 = holds no key, reads only; T1 = holds no key, builds nothing that can
move funds) are defined in [`docs/threat-model.md`](docs/threat-model.md#1-custody-tiers).

Every plugin is **channel-agnostic** — any ZeroClaw channel (Telegram, Discord, Matrix,
WhatsApp, email); demoed on Telegram — and **useful standalone from a laptop**:
`kiosk-watch` alone answers "is invoice X paid?", `kiosk-attest` alone notarizes
arbitrary readings, `kiosk-charge` alone issues Solana Pay requests.

### Also in the repo

| Path | What it is |
|---|---|
| [`config/example.toml`](config/example.toml) | Annotated operator config. No secrets — there are none to redact. |
| [`sops/`](sops) | Three ready-to-adapt Standard Operating Procedures (payment loop, sensor loop, heartbeat). |
| [`hardware/wiring.md`](hardware/wiring.md) | Pi 4 + relay + BME280 wiring, safety notes, calibration. |
| [`docs/threat-model.md`](docs/threat-model.md) | Custody tiers, trust boundaries, the full prompt-injection transcript. |
| [`SECURITY.md`](SECURITY.md) | Third-party trust surface: what ProofKiosk needs and, mostly, doesn't. |
| [`scripts/`](scripts) | devnet setup, wasm size report, and the no-network proof. |
| [`skills/kiosk-qr/`](skills/kiosk-qr) | Host-side QR + wallet tap-link rendering, kept out of the wasm to keep components small. |
| [`wit/v0/`](wit/v0) | Vendored ZeroClaw WIT world the plugins build against. |

## Tests & artifacts

All green, **no network in any test** — RPC is mocked through a one-method transport
seam, so `cargo test` never touches the wire.

| Component | Tests | Clippy `-D warnings` | wasm32-wasip2 |
|---|---|---|---|
| kiosk-core | 55 (incl. property + fuzz) | clean | — (rlib) |
| kiosk-charge | 12 | clean | 210 KB ✔ <250 KB |
| kiosk-watch | 24 | clean | 348 KB (bundles HTTP/TLS client) |
| kiosk-attest | 16 | clean | 384 KB (bundles HTTP/TLS client) |
| **total** | **107** | **clean** | `scripts/wasm-size.sh` |

The two RPC plugins are larger because a network-touching component must bundle an
HTTP/TLS client. That is inherent, not slack — and it is why `kiosk-charge`, which
touches nothing, is under half their size.

---

## The three-rung ladder — start with zero hardware

You do not need a Raspberry Pi, a sensor, or a relay to reproduce the core of
ProofKiosk. Each rung runs independently; **rung 1 is an evening on a laptop.**

### Rung 1 — laptop only

Prove the payment rail end to end against localnet or devnet:

1. `scripts/devnet-setup.sh` — spins up a validator (or targets devnet), mints a test
   USDC-like SPL token, and prints the config to paste.
2. Call `kiosk_charge` → get a `solana:` URL → pay it from any devnet wallet.
3. Call `kiosk_watch` with the returned `reference` → watch it flip `PENDING` → `PAID`.

No GPIO, no sensor. This is the full "ask for money → confirm money" loop.

### Rung 2 — add a sensor

Add a BME280 or any sensor tool. `kiosk-attest` writes each reading as a hash-chained,
durable-nonce memo transaction, so the environmental record is tamper-evident on-chain.
See [`sops/sensor-loop/`](sops/sensor-loop).

### Rung 3 — add a relay

Add a GPIO relay on a Raspberry Pi ([`hardware/wiring.md`](hardware/wiring.md)). The
payment-loop SOP fires the relay for exactly one condition: `kiosk_watch` returned a
verified payment.

> **Honest status:** rung 3 is **demo-wired, not production-wired.** The SOP validates
> and its routing is verified against the runtime, but the guard predicate
> (`$.steps.1.success`) does not resolve yet — the runtime's routing payload carries a
> step's output *string*, not the `ToolResult.success` boolean. The live behavior is the
> safe one (the relay stays shut), but the loop does not dispense as shipped. Full
> detail and the two ways to close it: [`sops/payment-loop/SOP.md`](sops/payment-loop/SOP.md).

---

## 5-minute quickstart (rung 1)

```bash
rustup target add wasm32-wasip2

# 1. Build and test everything (no network needed)
for d in crates/kiosk-core plugins/kiosk-charge plugins/kiosk-watch plugins/kiosk-attest; do
  (cd "$d" && cargo test)
done
for d in plugins/kiosk-*; do
  (cd "$d" && cargo build --target wasm32-wasip2 --release)
done

# 2. Prove kiosk-charge really has no network capability
./scripts/verify-no-network.sh

# 3. Get devnet/localnet values to paste into your config
./scripts/devnet-setup.sh
```

Then copy the blocks you need from [`config/example.toml`](config/example.toml) into
your ZeroClaw config, and in chat:

```
"sell a cold drink"   -> kiosk_charge returns a solana: URL (scan/tap, pay)
"is it paid?"         -> kiosk_watch flips PENDING -> PAID once it confirms
```

### Minimal config

Defaults keep the required surface tiny — one required key for charge, two for watch:

```toml
[plugins.kiosk-charge.config]
merchant_address = "YOUR_MERCHANT_PUBKEY"   # usdc_mint, cap, label all default

[plugins.kiosk-watch.config]
rpc_url          = "https://api.devnet.solana.com"
merchant_address = "YOUR_MERCHANT_PUBKEY"   # usdc_mint defaults to USDC, finality to "confirmed"
```

Full annotated config, including `kiosk-attest` and the `[sop]` block:
[`config/example.toml`](config/example.toml).

## Running the host

The stock ZeroClaw binary has **no plugin host**. Build the runtime from source with it
enabled:

```bash
# Laptop (rungs 1-2)
cargo build --release --features plugins-wasm,plugins-wasm-cranelift

# Raspberry Pi (rung 3) — adds the GPIO/peripheral tools for the relay
cargo build --release --features plugins-wasm,plugins-wasm-cranelift,hardware,peripheral-rpi
```

Validate the SOPs against your runtime before deploying them:

```bash
zeroclaw sop validate
```

## Proving the claims, not just asserting them

- **`scripts/verify-no-network.sh`** builds `kiosk-charge` for `wasm32-wasip2` and
  asserts its component imports **zero** `wasi:http` interfaces. It also prints
  `kiosk-watch`'s non-zero count — a read-only RPC client *should* import `wasi:http` —
  so the check is shown to discriminate rather than trivially pass.
- **Every fail-closed behavior is a host test.** 107 of them, RPC mocked, no network.
- **The attestation transaction is structurally incapable of moving funds** — Memo +
  System (advance-nonce) programs only, asserted against the built instruction set.

## Design constraints worth knowing

- **No `solana-sdk` / `solana-client`.** Neither compiles for `wasm32-wasip2`. base58,
  base64, shortvec, legacy message serialization, and the memo/nonce instruction
  builders are hand-rolled in `kiosk-core` with golden vectors.
- **Plugins are stateless.** The host creates a fresh store per call, so nothing is
  derived from static or thread-local state — counters and chain position come from
  config or from the chain itself.
- **Pure core + thin wasm shim.** Each plugin keeps its logic in a module with zero wasm
  dependencies plus a `#[cfg(target_family = "wasm")]` shim, which is what makes plain
  `cargo test` on the host possible.
- **Token-budgeted output.** Every `execute` output passes `kiosk_core::shape::clamp`,
  with a test asserting ≤ 200 tokens.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
