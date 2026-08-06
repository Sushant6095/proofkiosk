# ProofKiosk CI

The executable source of truth is [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).
Every push to `main` and every pull request runs the full gate; there are no path filters.
Jobs receive only `contents: read` permission.

## Build, test, and lint job

The first Ubuntu 24.04 job has a 30-minute timeout and runs:

1. Node 24.10.0 setup, `npm ci`, syntax validation of the independent Solana Pay
   transfer harness, 17 trusted handoff/claim/actuator tests, and `npm audit` at high severity.
2. Rust 1.97.1 with Clippy, rustfmt, and `wasm32-wasip2`.
3. `bash -n scripts/*.sh`, `scripts/host-infra-regression.sh`, and
   `scripts/check-config.sh config/example.toml`.
4. The 213 locked Rust tests: kiosk-core 80, kiosk-charge 19, kiosk-watch 76, and
   kiosk-attest 38. RPC is mocked in these tests; they do not contact Devnet.
5. Locked Clippy with warnings denied and rustfmt checks for all four crates.
6. Release component builds and enforced size ceilings via `scripts/wasm-size.sh`.
   Current gates are charge 220 KB / 250 KB, watch 390 KB / 400 KB, and attest
   418 KB / 450 KB.
7. Compiled-artifact inspection proving `kiosk-charge` imports zero `wasi:http`.

The Node and Rust suites contain 230 repository tests in total. The exact-host runtime
test below is a separate integration gate, not folded into that count.

## Exact pinned ZeroClaw job

The second Ubuntu 24.04 job has a 45-minute timeout. It builds the exact commit in
`wit/UPSTREAM_REF` with the checked-in upstream lockfile and
`plugins-wasm-cranelift`, using a cache keyed by the Rust version and ZeroClaw pin. It
then runs `scripts/host-smoke.sh`, which:

- uses an isolated temporary ZeroClaw config rather than the user's home config;
- stages, installs, and loads all three component packages;
- writes and reads the canonical natural-key `[[plugins.entries]]` config paths;
- validates all three SOP contracts; and
- transitively runs `scripts/exact-host-runtime-smoke.sh`.

The exact-runtime test instantiates all three components through ZeroClaw's real
`WasmTool` using the pinned source and lockfile. Deterministic local JSON-RPC fixtures
exercise valid business paths for all three: charge returns `created`; watch makes two
RPC calls and returns `paid`; attest reads a valid nonce account plus authenticated init
history and returns an unsigned `signature_required` message while asserting
`minContextSlot`. The test also proves host-injected config overrides caller-spoofed
`__config`, rejects unknown model-facing fields, and sends the actual host-direct charge
and paid-watch `ToolResult`s through trusted persistence, immutable economics/time
validation, one exclusive claim, and duplicate-claim rejection in an isolated directory.

This test does **not** contact public Devnet, sign an attestation, call an actuator, or
prove the headless SOP can dispatch ordinary plugin steps. `scripts/devnet-pay.mjs` is
also separate evidence: it submits and independently validates a Solana Pay-shaped test
transfer, but does not call `kiosk-watch`.

## Pinned and unpinned trust

- GitHub Actions are pinned by full commit SHA.
- Node, Rust, the `wasm32-wasip2` target, ZeroClaw Git commit, upstream Cargo lockfile,
  npm lockfile, and Solana Pay dependencies are pinned.
- Cargo crate lockfiles are independent because this repository has four standalone
  workspaces rather than one root Cargo workspace.
- CI does not provision a Solana validator, a public RPC, a wallet, a physical sensor,
  GPIO, or a signer. Those are manual/external integration boundaries.

## Local equivalent

From the repository root:

```bash
npm ci --ignore-scripts --no-audit --no-fund
npm run test:handoff
npm audit --omit=dev --audit-level=high

for d in crates/kiosk-core plugins/kiosk-charge plugins/kiosk-watch plugins/kiosk-attest; do
  (cd "$d" && cargo test --locked)
  (cd "$d" && cargo clippy --locked --all-targets -- -D warnings)
  (cd "$d" && cargo fmt --check)
done

bash -n scripts/*.sh
bash scripts/host-infra-regression.sh
./scripts/check-config.sh config/example.toml
./scripts/wasm-size.sh
./scripts/verify-no-network.sh
./scripts/install-pinned-zeroclaw.sh
export PATH="$PWD/.build/zeroclaw-install/bin:$PATH"
./scripts/host-smoke.sh
```

The last command proves exact-host loading/execution at the scope above. A complete
Devnet payment-verifier run and physical flow still require the operator procedure in
[`DEVNET-E2E-RUNBOOK.md`](DEVNET-E2E-RUNBOOK.md).
