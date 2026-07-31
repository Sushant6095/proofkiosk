# Payment loop

The core ProofKiosk safety loop: **money must be verified on-chain before anything
physical happens.**

## Flow

```
cron (every 10s)
   │
   ▼
1. kiosk_watch(reference, expected_amount, window_s)
   │
   ├── success == false  → 2. HOLD (terminal)  → PENDING / EXPIRED / MISMATCH / RPC error
   │                                             → nothing happens, poll again
   │
   └── success == true   → 3. relay_pulse(pin_ms=400) → 🥤 item dispensed
```

## Why it is safe

- Step 1's guard is `when: $.steps.1.success == "true"` with `next: 3`. This runtime
  routes a **false** guard to the next *linear* step, not past it — which is why step 2
  exists and is `terminal: true`. A guard that does not hold therefore lands on a step
  that does nothing and ends the branch; the relay at step 3 is reachable **only** by
  the explicit `next: 3` jump a true guard takes. Deleting step 2 would make a false
  guard fall through onto the relay, so it is load-bearing, not filler.
- `kiosk_watch` sets `success = true` **only** for a transaction that credits the exact
  `expected_amount` of the operator's USDC mint to the operator's address, referencing
  this charge, at the configured finality. Pending, expired, mismatch, and **RPC
  failure** all yield `success = false`.
- `deterministic = true` in `SOP.toml` means no LLM round-trip happens between the
  steps: the relay decision is made by the guard against structured step output, never
  by model prose.
- No `requires_confirmation` on the relay step is deliberate: the on-chain verification
  *is* the confirmation. There is no human in the actuation path, and there does not
  need to be.
- The agent holds no key. It cannot move funds; it can only read the chain and pulse a
  GPIO pin after the chain says paid.

## Adapting it

- `reference` / `expected_amount` come from the preceding `kiosk_charge` call for this
  sale. In a full deployment the charge step writes them into the run context and step 1
  reads them with a `{{steps.N.field}}` binding instead of the literals below.
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

1. **Verify payment on-chain** — Ask the chain whether the expected payment landed.
   - tools: kiosk_watch
   - allow-tools: kiosk_watch
   - call: {"tool":"kiosk_watch","args":{"reference":"REFERENCE_FROM_KIOSK_CHARGE","expected_amount":"1.5","window_s":300}}
   - when: $.steps.1.success == "true"
   - next: 3

2. **Hold — do not deliver** — Reached whenever the guard on step 1 does not hold
   (pending, expired, mismatch, or RPC failure). Ends the branch so the relay is never
   reached by fallthrough; the next cron tick polls again.
   - deny-tools: relay_pulse
   - terminal: true

3. **Dispense** — Pulse the delivery relay. Reachable only from step 1's true guard.
   - tools: relay_pulse
   - allow-tools: relay_pulse
   - call: {"tool":"relay_pulse","args":{"pin_ms":400}}
   - terminal: true
