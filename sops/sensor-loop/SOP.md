# Sensor loop

Turn a physical sensor into a tamper-evident on-chain record.

## Flow

```
cron (every 5 min)
   │
   ▼
1. bme280_read  →  { temp_c, humidity, ... }
   │
   ▼
2. kiosk_attest(kind="reading", metric="temp_c", value={{steps.1.temp_c}})
   │
   ▼
unsigned durable-nonce memo tx  →  signed & submitted by the operator signer
   │
   ▼
memo on-chain: {v, dev, seq, ts, metric, val, prev}  (seq/prev link the chain)
```

## Why it is trustworthy

- Each attestation memo carries `seq` and `prev` (the previous attestation's landed
  signature), so the readings form an ordered chain anchored on-chain. A missing or
  re-ordered reading is detectable by walking the chain.
- The transaction uses a **durable nonce** instead of a recent blockhash, so an
  attestation built now stays valid to submit later without a fresh blockhash — the Pi
  can attest even across brief connectivity gaps.
- `kiosk_attest` emits the transaction **unsigned** (zero signatures). The agent never
  signs; a separate operator signer does. The agent cannot forge or move anything.
- The attestation transaction contains **only** the Memo and System (advance-nonce)
  programs. A transfer is not expressible — this is enforced by a structural test in the
  plugin.
- `admission_policy = "hold"` serializes runs: two overlapping attestations would both
  read the same chain head and race on `seq`/`prev`.

## Notes on the bindings

- `{{steps.1.temp_c}}` pulls the reading out of step 1's output. A string that is
  exactly one binding resolves to the referenced JSON value, so `value` arrives as a
  number, which is what `kiosk_attest` expects.
- `ts` is deliberately omitted: this runtime has no `$.now` built-in, and
  `kiosk_attest` defaults `ts` to the current time when the argument is absent.
- `metric` must be present in the operator's `metric_allowlist` and the value inside its
  configured bounds, or the plugin refuses to attest rather than clamping a bad reading
  into a plausible lie.

## Steps

1. **Read the sensor** — Sample the environmental sensor.
   - tools: bme280_read
   - allow-tools: bme280_read
   - call: {"tool":"bme280_read","args":{}}

2. **Attest the reading** — Hash-chained, durable-nonce memo tx; output is unsigned.
   - tools: kiosk_attest
   - allow-tools: kiosk_attest
   - call: {"tool":"kiosk_attest","args":{"kind":"reading","metric":"temp_c","value":"{{steps.1.temp_c}}"}}
   - terminal: true
