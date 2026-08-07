# ProofKiosk — ZeroClaw × Solana Bounty Submission

> Submission status: form copy is ready. Add the final public video URL, publish the X post below and paste its URL, then confirm the video is **3:00 or shorter**. All other form fields are filled.

## Copy-paste bounty form

### Link to Your Submission

https://github.com/Sushant6095/proofkiosk

### Tweet Link

`[PASTE THE URL CREATED AFTER PUBLISHING THE X POST DRAFT BELOW]`

### Demo video link (YouTube/Vimeo/GDrive)

`[REPLACE WITH THE PUBLIC OR ANYONE-WITH-THE-LINK VIDEO URL — MAXIMUM 3:00]`

### One-pager link

https://proofkiosk.vercel.app/

### Supporting material

- Run all three plugins locally now: https://github.com/Sushant6095/proofkiosk/blob/main/RUN-LOCALLY-NOW.md
- Full repository README: https://github.com/Sushant6095/proofkiosk#readme
- Devnet end-to-end runbook: https://github.com/Sushant6095/proofkiosk/blob/main/docs/DEVNET-E2E-RUNBOOK.md
- Threat model: https://github.com/Sushant6095/proofkiosk/blob/main/docs/threat-model.md
- Hardware wiring and safety: https://github.com/Sushant6095/proofkiosk/blob/main/hardware/wiring.md
- Example ZeroClaw configuration: https://github.com/Sushant6095/proofkiosk/blob/main/config/example.toml
- Deterministic prompt-injection and fail-closed tests: https://github.com/Sushant6095/proofkiosk/tree/main/plugins
- CI definition: https://github.com/Sushant6095/proofkiosk/actions
- Public-Devnet transaction evidence: https://explorer.solana.com/tx/2C9v4EciqmpzCvCFyfkr5Vv5FmmxjnvDaEjTwsZh56zgfsdq4AM5sj4PhdF53jo6btB3fdRFGoFgVLyQpPZn8TZz?cluster=devnet
- Real ZeroClaw channel used in the demo: Discord

If the final video uses a newer transaction, replace the transaction link above before submitting.

### Anything Else?

ProofKiosk is a self-hosted, non-custodial Solana payment system that controls real hardware. The shopkeeper operates through a real ZeroClaw Discord channel; the customer sees a Raspberry Pi 4 kiosk, scans its QR, and signs with their own wallet. The physical build uses a 3.3V-compatible isolated relay, a separately powered 12V solenoid, flyback protection, fused wiring, and a fixed BCM17 output. An optional BME280 path supports environmental attestations.

Most payment agents stop at “paid.” ProofKiosk puts a deterministic boundary between payment and machinery. `kiosk-watch`, running as Rust/WASM inside the exact pinned ZeroClaw host, verifies execution, finality, recipient, mint, configured price, raw amount, token decimals, payer signer, reference, and the versioned `PKPAY1` order memo. Any mismatch fails closed. Even a valid paid result reports `actuation_authorized:false` and must cross a separate trusted host boundary.

That host matches the raw WIT result against the immutable order, wins one exclusive fulfillment claim, and only then permits one fixed 400 ms relay pulse. Replaying the same valid payment is refused and produces no second GPIO HIGH or solenoid movement. The language model cannot choose the recipient, mint, amount, RPC endpoint, GPIO pin, pulse duration, or cooldown.

Custody is T0/T1 only. No plugin contains a private-key, signing, transfer, refund, swap, or broadcast path; the customer wallet signs. The repository includes 213 Rust tests plus 24 Node boundary, actuator, and display tests—237 total—along with exact-host execution, permission and WASM-size gates, public-Devnet evidence, hardware safety documentation, and an evening-sized reproduction path. The demo honestly labels the asset as a Devnet test token and records `pulse_completed`, not `delivered`, because a delivery sensor is not installed yet.

Anyone can run all three plugins locally against the exact pinned ZeroClaw host right now—no wallet, public RPC, Raspberry Pi, or funds are required for the first verification: https://github.com/Sushant6095/proofkiosk/blob/main/RUN-LOCALLY-NOW.md

### KYC acknowledgement

- [x] I acknowledge that if I win, I will have to complete KYC verification to receive my prize money.

### X post draft

> Spent the last two weeks building ProofKiosk for the @SolanaBrasil ZeroClaw bounty, and it changed how I think about agents.
>
> I came in knowing software. I left understanding how software meets the physical world.
>
> Most payment demos stop at “paid.” ProofKiosk does not. After the customer signs, deterministic Rust/WASM verifies the exact finalized payment. A trusted host matches it to an immutable order and wins one exclusive claim before a Raspberry Pi 4 can pulse a real isolated relay and 12V solenoid. Replay the payment and there is no second pulse.
>
> The build uses a separate 12V supply, flyback protection, fused wiring, fixed BCM17 output, and an optional BME280 path for environmental attestations.
>
> The hard part was not making the relay click. It was making sure only the right payment could make it click, only once, without giving the agent a private key or direct GPIO authority.
>
> T0/T1 custody, 237 tests, and a complete demo: Discord → kiosk QR → wallet → Solana Devnet → ZeroClaw → Raspberry Pi → physical hardware.
>
> Huge thanks to @SuperteamBR and @SolanaBrasil. I learned more about embedded systems and secure agent design than I expected.
>
> Demo: [VIDEO URL]
> Repo: https://github.com/Sushant6095/proofkiosk
> One-pager: https://proofkiosk.vercel.app/

---

## Discord showcase post

### ProofKiosk — verified Solana payment before real hardware moves

Most payment agents stop at **“paid.”** ProofKiosk asks the harder question: when an AI agent touches a physical machine, what evidence is strong enough to let that machine move?

I built a self-hosted ZeroClaw kiosk around a Raspberry Pi 4, a 3.3V-compatible isolated relay, and a separately powered 12V solenoid with flyback protection and fused wiring. The shopkeeper operates through Discord. The customer never joins Discord—they see the Pi kiosk, scan its QR, and sign with their own wallet.

**The real flow shown in the video:**

1. ZeroClaw prepares one item-priced Solana Pay request from operator-owned policy.
2. The customer signs a public-Devnet test-token payment externally.
3. `kiosk-watch`, running as Rust/WASM in the exact pinned ZeroClaw host, verifies finality, execution, recipient, mint, raw amount, decimals, payer, reference, and the `PKPAY1` order memo.
4. A separate trusted host boundary matches the raw tool result to the immutable order and wins one exclusive fulfillment claim.
5. Only then does the Pi drive fixed BCM17 for 400 ms and click the physical relay.
6. I replay the same valid payment. The second claim is refused, GPIO never goes HIGH again, and the solenoid does not fire twice.

**What makes this different:** the LLM never decides that money moved, never owns a key, and never controls the pin. Model prose cannot actuate anything. The financial verifier and physical driver are separated by a raw structured result, an immutable order, and an atomic replay barrier.

**Custody:** T0/T1 only. No plugin has a private-key, signing, transfer, refund, swap, or broadcast path. The customer wallet signs. Operator config—not chat—owns the merchant, mint, prices, RPC endpoint, finality, GPIO pin, pulse length, and cooldown.

**Built and verified:** three bounded WASM tools, a shared pure Rust Solana core, trusted order persistence, the exclusive-claim driver, 213 Rust tests plus 24 Node boundary/actuator/display tests (**237 total**), exact-host execution, permission gates, and an evening-sized runbook.

**Honest limit:** the current evidence reaches `paid` → `claimed` → `pulse_completed`. I do not call that `delivered` because a physical delivery sensor is not installed yet. The optional BME280 path is for environmental attestations, not delivery confirmation.

**Links**

- Demo: `[FINAL VIDEO URL]`
- Repository: https://github.com/Sushant6095/proofkiosk
- One-pager: https://proofkiosk.vercel.app/
- Run locally now: https://github.com/Sushant6095/proofkiosk/blob/main/RUN-LOCALLY-NOW.md
- Reproduction runbook: https://github.com/Sushant6095/proofkiosk/blob/main/docs/DEVNET-E2E-RUNBOOK.md
- Threat model: https://github.com/Sushant6095/proofkiosk/blob/main/docs/threat-model.md
- Public-Devnet evidence: https://explorer.solana.com/tx/2C9v4EciqmpzCvCFyfkr5Vv5FmmxjnvDaEjTwsZh56zgfsdq4AM5sj4PhdF53jo6btB3fdRFGoFgVLyQpPZn8TZz?cluster=devnet

---

## Attach the plugins to ZeroClaw

### What ZeroClaw installs

Each ProofKiosk plugin is a `wasm32-wasip2` WebAssembly component implementing ZeroClaw's `wit/v0` tool-plugin world. Each staged plugin directory contains:

```text
staged/kiosk-charge/
├── manifest.toml
└── kiosk_charge.wasm

staged/kiosk-watch/
├── manifest.toml
└── kiosk_watch.wasm

staged/kiosk-attest/
├── manifest.toml
└── kiosk_attest.wasm
```

ZeroClaw reads `manifest.toml`, validates `wasm_path`, registers the component as one typed model-callable tool, and grants only the declared permissions:

| Package | Tool visible to ZeroClaw | Custody | Permissions |
|---|---|---:|---|
| `kiosk-charge` | `kiosk_charge` | T1 Build | `config_read` |
| `kiosk-watch` | `kiosk_watch` | T0 Read | `http_client`, `config_read` |
| `kiosk-attest` | `kiosk_attest` | T1 Build | `http_client`, `config_read` |

The plugins contain no private keys. `config_read` supplies only operator-owned public policy. `http_client` is used for read-only Solana JSON-RPC by watch/attest; charge has zero HTTP imports.

### 1. Clone and build the compatible ZeroClaw host

The normal release binary used during development omitted the WASM plugin runtime. ProofKiosk pins the exact compatible ZeroClaw source revision in `wit/UPSTREAM_REF` and builds it with `plugins-wasm-cranelift`:

```bash
git clone https://github.com/Sushant6095/proofkiosk.git
cd proofkiosk

./scripts/install-pinned-zeroclaw.sh
export PATH="$PWD/.build/zeroclaw-install/bin:$PATH"

zeroclaw --version
zeroclaw plugin --help
```

For the Raspberry Pi hardware host:

```bash
ZEROCLAW_FEATURES=plugins-wasm-cranelift,hardware,peripheral-rpi \
  ./scripts/install-pinned-zeroclaw.sh

export PATH="$PWD/.build/zeroclaw-install/bin:$PATH"
```

The ProofKiosk actuator is intentionally outside the agent and WASM sandbox. ZeroClaw cannot call arbitrary GPIO; the host-side process accepts only verified raw evidence plus the immutable order.

### 2. Build, test, and stage all three components

```bash
rustup target add wasm32-wasip2
npm ci
npm test

for manifest in \
  crates/kiosk-core/Cargo.toml \
  plugins/kiosk-charge/Cargo.toml \
  plugins/kiosk-watch/Cargo.toml \
  plugins/kiosk-attest/Cargo.toml
do
  cargo test --manifest-path "$manifest" --locked
done

./scripts/stage-plugin.sh
```

Expected repository test surface: 213 Rust tests + 24 Node trusted-boundary/actuator/display tests = 237.

### 3. Install and enable the plugins

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

All three must appear as WASM tools. A missing plugin is a failed installation; do not continue until `plugin info` identifies the manifest or ABI error.

### 4. Add operator-owned configuration

For the shortest safe Devnet setup, let the repository create throwaway customer/merchant wallets, a test mint, a fresh reference, a durable nonce account, and a validated ZeroClaw configuration:

```bash
MODE=devnet MINT_AMOUNT=20 ./scripts/devnet-setup.sh
source .devnet/payment.env

./scripts/check-config.sh "$PROOFKIOSK_CONFIG"
```

The generated `.devnet/zeroclaw.toml` contains the three canonical `[[plugins.entries]]` rows. For a persistent operator installation, copy those rows into the active ZeroClaw config or set their values through `zeroclaw config set`.

Minimum cross-plugin invariants:

- `kiosk-charge.merchant_address == kiosk-watch.merchant_address`
- `kiosk-charge.usdc_mint == kiosk-watch.usdc_mint`
- `kiosk-charge.token_decimals == kiosk-watch.token_decimals`
- `kiosk-charge.price_list == kiosk-watch.price_list`
- `kiosk-watch.device_address == kiosk-attest.nonce_account`
- `kiosk-watch.device_authority == kiosk-attest.nonce_authority`
- `kiosk-watch.device_id == kiosk-attest.device_id`
- all RPC URLs, mints, nonce accounts, and wallets belong to the same Solana cluster

No tool argument may override these values. ZeroClaw injects only the installed plugin's own config as `__config`; caller-supplied `__config` is rejected/overridden at the host boundary.

### 5. Verify that ZeroClaw actually executes the plugins

```bash
./scripts/verify-no-network.sh
./scripts/wasm-size.sh
./scripts/check-config.sh "$PROOFKIOSK_CONFIG"
./scripts/host-smoke.sh
```

`host-smoke.sh` uses an isolated config, installs all three staged components, invokes them through the exact pinned ZeroClaw `WasmTool`, validates all SOP contracts, persists the actual charge result, accepts the actual paid-watch result once, and rejects a duplicate claim.

### 6. Run the payment use case

Source the isolated demo environment in every new shell:

```bash
source scripts/demo-env.sh
```

Through the configured ZeroClaw channel, the operator asks the agent to sell an allowlisted item. ZeroClaw discovers and calls:

```json
{"item_id":"cold_drink"}
```

on `kiosk_charge`. The returned raw ToolResult is passed to the trusted handoff, which validates the Solana Pay URI and snapshots the immutable order before rendering the QR.

After the customer wallet pays, ZeroClaw calls `kiosk_watch` with only:

```json
{"reference":"THE_CHARGE_REFERENCE","item_id":"cold_drink"}
```

There is deliberately no amount, recipient, mint, RPC URL, commitment, or GPIO argument. The paid verdict is usable by the actuator only when the raw host result matches the persisted order and `requires_atomic_claim` is true.

The complete public-Devnet and hardware capture procedure is in [`docs/DEVNET-E2E-RUNBOOK.md`](docs/DEVNET-E2E-RUNBOOK.md) and the generated rehearsal guide in `deliverables/`.

### 7. Connect SOPs and the real channel

```bash
zeroclaw config set --no-interactive sop.sops_dir "$PWD/sops"
zeroclaw config set --no-interactive sop.step_scope_enforce true

zeroclaw sop list
zeroclaw sop validate proofkiosk-payment-loop
zeroclaw sop validate proofkiosk-sensor-loop
zeroclaw sop validate proofkiosk-heartbeat
```

Bind the channel actually shown in the final video using ZeroClaw's normal channel configuration, restrict it to the operator identity, and run the same charge → external wallet payment → finalized watch flow. The plugins are channel-blind: Telegram, Discord, WhatsApp, Matrix, email, webhook, or CLI messages reach the same typed tools and the same host-owned policy.

Do not claim that SOP validation alone executes GPIO. The current bounded actuator is a separate host-side program and must run on the Raspberry Pi after the raw host-direct result crosses the trusted order/claim boundary.

---

## Final pre-submission gates

- [ ] Final video is **2:59 or shorter**, contains no slides, and shows a real ZeroClaw channel.
- [ ] The same fresh reference is visible across charge, wallet payment, Explorer, raw watch result, immutable order, claim, and actuator journal.
- [ ] The video labels the asset as a public-Devnet test token, not real USDC or mainnet money.
- [ ] The hardware output is shown in the same continuous actuation shot as the real Pi terminal output.
- [ ] Replaying the paid result visibly fails and does not produce a second HIGH pulse.
- [ ] The narration says `pulse_completed`, not `delivered`, unless a physical sensor proves delivery.
- [ ] The real channel and operator allowlist are named in the write-up.
- [ ] The prompt-injection transcript is linked and shows no tool call capable of refunding, redirecting, signing, or changing GPIO policy.
- [ ] Repository changes, this file, config, SOPs, tests, and the final transaction evidence are pushed to the public branch linked above.
- [ ] Tweet URL, demo video URL, and final Explorer transaction URL replace every placeholder in this file.
