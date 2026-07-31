# Heartbeat

A liveness watchdog for the kiosk's attestation stream — the inverse gate of the
payment loop.

## Flow

```
cron (every 10 min)
   │
   ▼
1. kiosk_watch(mode="heartbeat", device_address, max_silence_s=1800)
   │
   ├── success == true   → 2. LIVE (terminal) → newest attestation is fresh, do nothing
   │
   └── success == false  → 3. notify_operator(...)  → STALE or MISSING
```

## What it catches

- **STALE** — the device is still on-chain but hasn't attested within
  `max_silence_s` (here, 30 min): sensor hung, connectivity lost, or the attestation
  loop crashed.
- **MISSING** — no attestations found at all for the device address: never provisioned,
  or wrong address configured.

Both are operational failures the operator wants to know about immediately, so the
alert step is the one reached on `success == false` — the mirror image of the payment
loop, which acts on `success == true`.

## Routing note

Same shape as the payment loop, inverted. Step 1's guard is
`when: $.steps.1.success == "false"` with `next: 3`. A guard that does not hold falls
through to the *linear* next step, which is why step 2 exists and is `terminal: true`:
"healthy" must land somewhere that ends the branch. The alert at step 3 is reachable
only via the explicit jump.

## Known gap

Same as the payment loop: `$.steps.1.success` does not resolve today, because the
routing payload is built from the step's `output` **string** and `kiosk_watch` reports
its verdict in the separate `ToolResult.success` boolean. Unresolved paths evaluate
false, so a stale device would currently take the "live" branch and **not** alert. See
`sops/payment-loop/SOP.md` for the two ways to close it. The file validates and the
routing is correct; the predicate is the missing piece.

## Adapting it

- Point `device_address` at the same address `kiosk_attest` writes its chain to.
- Tune `max_silence_s` to a small multiple of your sensor-loop cadence (e.g. 6× a 5-min
  loop = 30 min) so one missed reading doesn't page you, but a dead device does.
- `notify_operator` stands in for whatever channel plugin you use (Telegram, email, …).
  Replace the tool name and args to match it.

## Steps

1. **Check attestation freshness** — Heartbeat mode; no payment is involved.
   - tools: kiosk_watch
   - allow-tools: kiosk_watch
   - call: {"tool":"kiosk_watch","args":{"mode":"heartbeat","device_address":"DEVICE_ATTESTATION_PUBKEY","max_silence_s":1800}}
   - when: $.steps.1.success == "false"
   - next: 3

2. **Live — nothing to do** — The newest attestation is inside the silence window.
   Ends the branch so a healthy check never falls through onto the alert.
   - terminal: true

3. **Alert the operator** — Reached only when the heartbeat is stale or missing.
   - tools: notify_operator
   - allow-tools: notify_operator
   - call: {"tool":"notify_operator","args":{"message":"ProofKiosk heartbeat STALE/MISSING — check device."}}
   - terminal: true
