# Heartbeat

A liveness watchdog for the kiosk's attestation stream — the inverse gate of the
payment loop.

## Flow

```
declared cron / external driver (every 10 min)
   │
   ▼
1. kiosk_watch(mode="heartbeat")
   │
   ├── success == true   → 2. LIVE (terminal) → newest attestation is fresh, do nothing
   │
   └── success == false  → 3. notify_operator(...)  → STALE or MISSING
```

## What it catches

- **STALE** — the device is still on-chain but hasn't attested within
  operator-configured `heartbeat_max_silence_s` (here, 30 min): sensor hung,
  connectivity lost, or the attestation
  loop crashed.
- **MISSING** — no attestations found at all for the device address: never provisioned,
  or wrong address configured.

Both are operational failures the operator wants to know about immediately, so the
alert step is the one reached on `success == false` — the mirror image of the payment
loop, which acts on `success == true`.

## Routing note

Same shape as the payment loop, inverted. `kiosk_watch` now emits structured JSON, so
`$.steps.1.success` is present and false only for `stale`/`missing`. At the exact pinned
host, a false top-level guard completes instead of taking a guarded jump; do not infer a
self-running notification flow from validation alone.

Configuration, RPC, and decode errors are failed WIT results with empty output, not a
`stale` JSON object. An external driver must treat either a business-negative heartbeat
or execution failure as unhealthy and notify the operator; it must never invent JSON to
make the SOP predicate run.

## Known gap

The former predicate-data gap is fixed, and heartbeat candidates now require the
operator-configured authority signer, device account, and exact device id. The remaining
runtime gap is headless dispatch: exact pinned deterministic execution does not
self-invoke ordinary plugin/tool steps, and `notify_operator` is a placeholder this repo
does not ship. Use an external driver and channel adapter; validation proves syntax only.

## Adapting it

- In the `kiosk-watch` config row, set `device_address` and `device_id` to the same nonce
  account and id configured for `kiosk-attest`; callers cannot override them.
- Tune operator config `heartbeat_max_silence_s` to a small multiple of your sensor-loop
  cadence (e.g. 6× a 5-min loop = 30 min) so one missed reading doesn't page you, but a
  dead device does.
- `notify_operator` stands in for whatever channel plugin you use (Telegram, email, …).
  Replace the tool name and args to match it.

## Steps

1. **Check attestation freshness** — Heartbeat mode; no payment is involved.
   - tools: kiosk_watch
   - allow-tools: kiosk_watch
   - call: {"tool":"kiosk_watch","args":{"mode":"heartbeat"}}
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
