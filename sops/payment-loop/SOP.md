# Payment loop

The core ProofKiosk safety loop: **money must be verified on-chain before anything
physical happens, and a charge must only be delivered once.**

## Target flow (external driver required)

```
timer / order driver (every 10s)
   │
   ▼
1. kiosk_watch(reference, item_id)
   │        price comes from operator config, keyed by item_id — never from the model
   │        also: returns ALREADY FULFILLED if this charge already has a marker
   │
   ├── business success == false → HOLD         → PENDING / MISMATCH /
   │                                             ALREADY FULFILLED
   ├── failed WIT result → HOLD                  → config / RPC / decode error
   │                                             → nothing happens, poll again
   │
   └── paid + raw host-direct result
                          → validate persisted order + exclusive local claim
                          → safe relay adapter (target pulse: 400 ms)
                                  │
                                  ▼
                           4. kiosk_attest(kind=fulfillment, reference)
                                  → unsigned PKFUL1 marker → operator signer submits it
                                  → step 1 can return ALREADY FULFILLED while the
                                    authenticated marker stays in the 10-signature window
```

This is the intended contract, not a claim that the checked-in SOP runs the whole loop
headlessly. At the exact compatible ZeroClaw pin, deterministic headless execution
self-dispatches only `capability` steps. The ordinary `kiosk_watch` and `kiosk_attest`
plugin calls below require an external driver, and `relay_pulse` is not a tool shipped by
this repository.

## Safety properties in the contract

- Step 1's guard reads `$.steps.1.success` from `kiosk_watch`'s structured JSON output.
  The plugin emits `true` only for `status="paid"`; every other verdict emits `false`.
  At the exact pin, a false top-level guard completes rather than taking the guarded
  `next:` jump, so it cannot fall through into a relay step. Step 2 remains an explicit
  documentation/driver hold state, not the sole safety barrier.
- `kiosk_watch` sets inner `success = true` **only** for a transaction that credits the
  **operator-configured price of `item_id`** in the operator's USDC mint to the
  operator's address, referencing this charge, at the configured finality — and only
  when no authenticated fulfillment marker for this charge already exists. Pending,
  mismatch, and already-fulfilled yield inner `success = false`. Config, RPC,
  and decode failures instead produce a failed WIT result with empty output. Both forms
  must hold.
- **There is no amount argument.** The price is read from the `kiosk-watch`
  `[[plugins.entries]]` row's `[plugins.entries.config]`, keyed by an item id the caller may choose but never
  write. A compromised model can name the wrong item; it cannot name a wrong price.
- `deterministic = true` means the routing decision itself needs no LLM round-trip. It
  does **not** provide a driver for ordinary plugin tools or a hardware adapter.
- A production driver must accept only the raw host-direct result, validate it against
  the persisted trusted order, create the exclusive host-local claim, and only then call
  a bounded pulse adapter. Never pass model prose into this boundary.
- The agent holds no key and does not itself pulse GPIO. The external driver and signer
  remain separate trust boundaries.

## On-chain marker and local claim

A verified payment stays verified forever. Without a fulfillment marker, every
subsequent driver poll can re-verify the same charge and pulse the relay again — the
plugin is stateless by construction (fresh WASI store per call, no statics), so it
cannot remember that it already dispensed. Single-use is enforced by reading the
marker back off the chain, which means **the marker has to actually get written.**

Step 4 emits an *unsigned* transaction. It is meaningful only once your operator signer
signs and submits it, so:

- **Automate and constrain the signer.** A signer sidecar should independently decode the
  artifact, enforce Memo + System-only policy, submit it, and wait for `finalized`. That
  sidecar is not implemented in this repo; a human leaves the replay window open.
- **The reference scan is bounded.** More than ten newer writes can hide the marker, so
  the on-chain marker cannot be the only physical replay guard. The local exclusive
  claim remains mandatory.

## At-most-once claim is not exactly-once delivery

`trusted-charge-handoff.mjs` durably creates the order from a raw host-direct charge
result. `trusted-order-claim.mjs` validates a raw host-direct paid result against that
order and uses exclusive creation so a second claim fails. This closes duplicate action
on one correctly integrated host.

It does not solve the other side of the crash window: if the driver claims and then dies
before pulsing, the customer is paid but undelivered. A physical system still needs a
claimed → actuating → delivered journal, delivery sensor, and explicit recovery/operator
policy. None of that driver/hardware code is shipped.

## Adapting it

- `reference` / `item_id` must come from the trusted order persisted from the preceding
  raw host-direct `kiosk_charge` call. The helpers are shipped; the checked-in SOP still
  contains literals because no headless external driver binds the record into the steps.
- `item_id` must exist in **both** `price_list` blocks. `scripts/check-config.sh`
  verifies the two agree, along with `device_authority == nonce_authority` — get that
  second one wrong and no marker ever authenticates, so the on-chain replay barrier
  stops working. The local exclusive claim remains mandatory either way.
- `finality` is already `finalized` and cannot be lowered for a payment verdict — the
  weaker commitments are refused rather than configurable. Budget ~13 s from payment to
  verdict; that is the cost of not dispensing against a transaction that can be rolled
  back.
- `pin_ms` and the relay adapter depend on your hardware wiring. The pinned host exposes
  lower-level GPIO support when built with `hardware,peripheral-rpi`, but ProofKiosk does
  not currently implement the named `relay_pulse` tool, enforced maximum duration,
  startup-low behavior, or delivery sensor (see `hardware/wiring.md`).

## Known gaps — read before integrating this loop

`zeroclaw sop validate` passes, and the former guard-data gap is closed:
`kiosk_watch` emits a complete JSON object containing `success` and `status`, which the
routing payload can parse. The remaining gaps are larger than a predicate typo:

1. **No headless plugin dispatcher.** At commit
   `e112ce6b5ccdac9e1cb166bab217e730dd7e24c2`, deterministic headless runs execute
   `capability` steps themselves; ordinary plugin/tool steps report that an external
   driver is required. A cron declaration therefore does not poll this plugin by itself.
2. **No shipped orchestration/recovery driver.** Trusted persistence and one exclusive
   host-local claim exist, but `REFERENCE_FROM_KIOSK_CHARGE` and `cold_drink` remain SOP
   literals. No checked-in driver captures the raw host-direct paid result, claims the
   order, invokes the actuator, journals physical state, and recovers after a crash.
3. **No pulse adapter.** `relay_pulse` is a desired narrow tool contract, not an
   installed tool in this repo or in the pinned host.
4. **No signer/submission/finality loop.** Step 4 produces unsigned message bytes only.
5. **Not exactly-once physical delivery.** Exclusive claim prevents a second claim on one
   host, but a crash after claim can leave the item undelivered; no delivery sensor or
   recovery policy exists.

Use this SOP as a reviewable orchestration specification. For a real demo, drive the
plugin calls explicitly and show the structured outputs; do not claim the cron trigger
or physical delivery is autonomous until those five integration gaps are closed.

## Steps

1. **Verify payment on-chain** — Ask the chain whether the expected payment landed, and
   whether this charge was already delivered.
   - tools: kiosk_watch
   - allow-tools: kiosk_watch
   - call: {"tool":"kiosk_watch","args":{"reference":"REFERENCE_FROM_KIOSK_CHARGE","item_id":"cold_drink"}}
   - when: $.steps.1.success == "true"
   - next: 3

2. **Hold — do not deliver** — Explicit state for an external driver whenever the
   verdict is pending, mismatch, already fulfilled, or an RPC failure. The exact
   pinned top-level false guard completes before this step, so production code must not
   rely on this prose step for polling behavior.
   - deny-tools: relay_pulse
   - terminal: true

3. **Claim then dispense (desired driver + adapter)** — Before this step, the external
   driver must validate the raw host-direct paid result and successfully run
   `trusted-order-claim.mjs`. Only then may it pulse the relay. `relay_pulse` is not
   shipped; an integration must implement its bounds and fail-safe behavior.
   - tools: relay_pulse
   - allow-tools: relay_pulse
   - call: {"tool":"relay_pulse","args":{"pin_ms":400}}

4. **Record the fulfillment** — Build the unsigned PKFUL1 marker naming this charge.
   Once the operator signer submits it, step 1 can return ALREADY FULFILLED while the
   authenticated marker remains inside the bounded scan. The local exclusive claim is
   still the physical replay barrier.
   - tools: kiosk_attest
   - allow-tools: kiosk_attest
   - call: {"tool":"kiosk_attest","args":{"kind":"fulfillment","reference":"REFERENCE_FROM_KIOSK_CHARGE","item":"cold_drink"}}
   - terminal: true
