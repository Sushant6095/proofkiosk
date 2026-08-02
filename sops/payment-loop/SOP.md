# Payment loop

The core ProofKiosk safety loop: **money must be verified on-chain before anything
physical happens, and a charge must only be delivered once.**

## Flow

```
cron (every 10s)
   │
   ▼
1. kiosk_watch(reference, item_id, window_s)
   │        price comes from operator config, keyed by item_id — never from the model
   │        also: returns ALREADY FULFILLED if this charge already has a marker
   │
   ├── success == false  → 2. HOLD (terminal)  → PENDING / EXPIRED / MISMATCH /
   │                                             ALREADY FULFILLED / RPC error
   │                                             → nothing happens, poll again
   │
   └── success == true   → 3. relay_pulse(pin_ms=400) → 🥤 item dispensed
                                  │
                                  ▼
                           4. kiosk_attest(kind=fulfillment, reference)
                                  → unsigned PKFUL1 marker → operator signer submits it
                                  → step 1 returns ALREADY FULFILLED from then on
```

## Why it is safe

- Step 1's guard is `when: $.steps.1.success == "true"` with `next: 3`. This runtime
  routes a **false** guard to the next *linear* step, not past it — which is why step 2
  exists and is `terminal: true`. A guard that does not hold therefore lands on a step
  that does nothing and ends the branch; the relay at step 3 is reachable **only** by
  the explicit `next: 3` jump a true guard takes. Deleting step 2 would make a false
  guard fall through onto the relay, so it is load-bearing, not filler.
- `kiosk_watch` sets `success = true` **only** for a transaction that credits the
  **operator-configured price of `item_id`** in the operator's USDC mint to the
  operator's address, referencing this charge, at the configured finality — and only
  when no authenticated fulfillment marker for this charge already exists. Pending,
  expired, mismatch, already-fulfilled, and **RPC failure** all yield `success = false`.
- **There is no amount argument.** The price is read from `price_list` in
  `[plugins.kiosk-watch.config]`, keyed by an item id the caller may choose but never
  write. A compromised model can name the wrong item; it cannot name a wrong price.
- `deterministic = true` in `SOP.toml` means no LLM round-trip happens between the
  steps: the relay decision is made by the guard against structured step output, never
  by model prose.
- No `requires_confirmation` on the relay step is deliberate: the on-chain verification
  *is* the confirmation. There is no human in the actuation path, and there does not
  need to be.
- The agent holds no key. It cannot move funds; it can only read the chain, pulse a
  GPIO pin after the chain says paid, and build an unsigned receipt someone else signs.

## Step 4 is not optional

A verified payment stays verified forever. Without a fulfillment marker, every
subsequent cron tick re-verifies the same charge and pulses the relay again — the
plugin is stateless by construction (fresh WASI store per call, no statics), so it
cannot remember that it already dispensed. Single-use is enforced by reading the
marker back off the chain, which means **the marker has to actually get written.**

Step 4 emits an *unsigned* transaction. It is delivered only once your operator signer
signs and submits it, so:

- **Automate the signer.** A co-located signing daemon lands the marker in a second or
  two — one or two cron ticks. A human in that path leaves the window open indefinitely.
- **Between the relay pulse and the marker confirming, `kiosk_watch` still says PAID.**
  That is deliberate: the loop is **at-least-once**. See below.

## At-least-once, and when that is the wrong choice

Ordering is relay-then-marker. If the marker write fails, the charge can be delivered
again on a later tick. The alternative — marker first, relay second — trades that for
the opposite failure: a customer who paid gets nothing because the signer was down.

For the actuators this loop targets, re-firing is harmless:

| Actuator | Re-fire means | Verdict |
|---|---|---|
| Door / locker latch | it unlocks again | harmless |
| EV charger enable | it re-enables an already-enabled session | harmless |
| Turnstile, gate | opens again for the same buyer | harmless |
| **Vending / consumable dispenser** | **a second drink drops, unpaid** | **not acceptable** |

**A consumable dispenser needs at-most-once and this loop does not provide it.** Do not
wire this SOP to one without changing the ordering to marker-first plus an operator
retry path for the "paid but not delivered" case. That policy is not implemented here;
saying so is more useful than a config flag that pretends the trade-off went away.

## Adapting it

- `reference` / `item_id` come from the preceding `kiosk_charge` call for this sale. In
  a full deployment the charge step writes them into the run context and step 1 reads
  them with a `{{steps.N.field}}` binding instead of the literals below.
- `item_id` must exist in **both** `price_list` blocks. `scripts/check-config.sh`
  verifies the two agree, along with `device_authority == nonce_authority` — get that
  second one wrong and no marker ever authenticates, so single-use silently stops
  working.
- Raise `finality` to `finalized` in `[plugins.kiosk-watch.config]` if you want economic
  irreversibility before dispensing (adds ~13s).
- `pin_ms` and the relay tool name depend on your hardware wiring (see the Pi build:
  `--features hardware,peripheral-rpi`, and `hardware/wiring.md`).

## Known gap — read before deploying this loop

`zeroclaw sop validate` passes on this file, and the *routing* above is verified against
the runtime's `resolve_next` (a false top-level `when` guard bypasses `next:` and takes
the linear successor — `sop/route/mod.rs`, test `when_false_advances_to_linear_successor`).

What is **not** yet wired is the guard's left-hand side. The routing payload is built
from each step's `SopStepResult.output`, which is a **string** (parsed as JSON only if
the whole string happens to parse). `kiosk_watch` returns its verdict as the separate
`ToolResult.success` boolean and puts human-readable prose in `output`, so
`$.steps.1.success` does not resolve today. Condition evaluation is fail-closed on an
unresolved path, so the effect is the safe one — the run lands on step 2 and the relay
stays shut — but it means **this loop will not dispense as written**. It is a correct,
validated skeleton, not a live actuation path.

Closing it needs one of:

- a machine-readable verdict in `kiosk_watch`'s `output` (a JSON object the step's
  `output:` contract can validate), which is a plugin-output change, or
- a host change that surfaces `ToolResult.success` into the routing payload.

Either way the guard direction stays the same: the relay is reachable only from an
affirmative verdict. Until then, treat rung 3 as demo-wired, not production-wired.

## Steps

1. **Verify payment on-chain** — Ask the chain whether the expected payment landed, and
   whether this charge was already delivered.
   - tools: kiosk_watch
   - allow-tools: kiosk_watch
   - call: {"tool":"kiosk_watch","args":{"reference":"REFERENCE_FROM_KIOSK_CHARGE","item_id":"cold_drink","window_s":300}}
   - when: $.steps.1.success == "true"
   - next: 3

2. **Hold — do not deliver** — Reached whenever the guard on step 1 does not hold
   (pending, expired, mismatch, already fulfilled, or RPC failure). Ends the branch so
   the relay is never reached by fallthrough; the next cron tick polls again.
   - deny-tools: relay_pulse
   - terminal: true

3. **Dispense** — Pulse the delivery relay. Reachable only from step 1's true guard.
   Falls through to step 4, which is what makes the delivery single-use.
   - tools: relay_pulse
   - allow-tools: relay_pulse
   - call: {"tool":"relay_pulse","args":{"pin_ms":400}}

4. **Record the fulfillment** — Build the unsigned PKFUL1 marker naming this charge.
   Once the operator signer submits it, step 1 returns ALREADY FULFILLED for this
   reference and the relay cannot fire for it again.
   - tools: kiosk_attest
   - allow-tools: kiosk_attest
   - call: {"tool":"kiosk_attest","args":{"kind":"fulfillment","reference":"REFERENCE_FROM_KIOSK_CHARGE","item":"cold_drink"}}
   - terminal: true
