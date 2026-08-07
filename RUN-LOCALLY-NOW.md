# Run ProofKiosk locally now

You can build, install, and execute all three ProofKiosk WASM plugins through the exact pinned ZeroClaw host without a wallet, public RPC endpoint, Raspberry Pi, or real funds.

This first run is a deterministic local verification. It proves the components compile, load through ZeroClaw's real WIT boundary, receive host-owned configuration, execute their valid business paths, persist an immutable order, accept one exclusive claim, and reject its replay.

## Prerequisites

- Git and Bash
- Rust through `rustup`
- Node.js 24+
- A C/C++ build toolchain suitable for compiling ZeroClaw

## One-command-path verification

```bash
git clone https://github.com/Sushant6095/proofkiosk.git
cd proofkiosk

rustup target add wasm32-wasip2
npm ci --ignore-scripts --no-audit --no-fund

./scripts/install-pinned-zeroclaw.sh
export PATH="$PWD/.build/zeroclaw-install/bin:$PATH"

./scripts/host-smoke.sh
```

The installer builds ZeroClaw commit `e112ce6b5ccdac9e1cb166bab217e730dd7e24c2` with `plugins-wasm-cranelift`. The smoke test then:

1. builds and stages `kiosk-charge`, `kiosk-watch`, and `kiosk-attest` for `wasm32-wasip2`;
2. installs all three into a temporary isolated ZeroClaw configuration;
3. confirms ZeroClaw recognizes each component as a WASM tool;
4. validates the three ProofKiosk SOP contracts;
5. invokes charge, paid-watch, and unsigned-attestation paths through the real pinned host boundary;
6. carries the raw charge/watch results through trusted order persistence;
7. accepts one exclusive fulfillment claim and rejects the duplicate claim.

An exit code of zero is the local exact-host proof. This run uses deterministic local RPC fixtures: it does not contact public Devnet, move funds, sign a transaction, or energize GPIO.

## Inspect or install the staged plugins

`host-smoke.sh` stages the installable bundles under:

```text
staged/kiosk-charge/
staged/kiosk-watch/
staged/kiosk-attest/
```

To install them into a separate plugin-capable ZeroClaw test configuration:

```bash
zeroclaw plugin install ./staged/kiosk-charge/
zeroclaw plugin install ./staged/kiosk-watch/
zeroclaw plugin install ./staged/kiosk-attest/

zeroclaw config set --no-interactive plugins.enabled true
zeroclaw config set --no-interactive plugins.auto_discover true

zeroclaw plugin list --all
zeroclaw plugin info kiosk-charge
zeroclaw plugin info kiosk-watch
zeroclaw plugin info kiosk-attest
```

Start from [`config/example.toml`](config/example.toml) for operator-owned merchant, mint, catalog, RPC, nonce, and device values. Then validate the cross-plugin invariants:

```bash
./scripts/check-config.sh config/example.toml
```

Never place a private key or seed phrase in ZeroClaw or plugin configuration. ProofKiosk is T0/T1: the customer or external operator wallet remains the signer.

## Run the full test surface

```bash
for manifest in \
  crates/kiosk-core/Cargo.toml \
  plugins/kiosk-charge/Cargo.toml \
  plugins/kiosk-watch/Cargo.toml \
  plugins/kiosk-attest/Cargo.toml
do
  cargo test --manifest-path "$manifest" --locked
done

npm run test:handoff
```

Expected repository surface: 213 Rust tests plus 24 Node trusted-boundary, actuator, and display tests—237 total. The exact-host smoke and shell infrastructure regression are separate gates.

## Continue to Devnet or Raspberry Pi hardware

- Public/local Solana payment walkthrough: [`docs/DEVNET-E2E-RUNBOOK.md`](docs/DEVNET-E2E-RUNBOOK.md)
- Raspberry Pi wiring and electrical safety: [`hardware/wiring.md`](hardware/wiring.md)
- Custody and failure analysis: [`docs/threat-model.md`](docs/threat-model.md)

The hardware driver records `pulse_completed`, not `delivered`. Do not connect an energized load until the GPIO level, relay polarity, flyback protection, fuse, separate supply, and common-ground/isolation requirements have been checked on the actual hardware.
