# ProofKiosk SOPs

These directories are **integration contracts** for payment polling, sensor readings,
and heartbeat checks. They document the intended deterministic branches and validate on
the exact compatible ZeroClaw host commit
`e112ce6b5ccdac9e1cb166bab217e730dd7e24c2` (source version 0.8.2).

They are not evidence of a self-running kiosk.

## Runtime reality

| Layer | Current state |
|---|---|
| Parse and validate all three SOPs | Implemented; `host-smoke.sh` asserts the exact loaded ID set and validates each ID explicitly. |
| Route `kiosk_watch` output | Implemented. Payment and heartbeat results are complete JSON objects with `success` and `status`. |
| Headless cron invokes ordinary plugin steps | **Not implemented by the pinned host.** Deterministic headless execution self-dispatches `capability` steps; ordinary plugin/tool steps require an external driver. |
| Trusted order/reference persistence | **Shipped host-side.** Raw host-direct charge output is validated against config and persisted durably; the checked-in SOP still contains placeholders and no driver wires the record into it. |
| Single-host order claim | **Shipped host-side.** A raw host-direct paid result can create one exclusive claim; this is at-most-once claiming, not exactly-once physical delivery. |
| `relay_pulse` hardware adapter | **Not shipped.** It is a desired narrow interface, not an installed ZeroClaw tool. |
| Attestation signing/submission/finality | **Not shipped.** `kiosk_attest` stops at `status="signature_required"` with unsigned message bytes. |
| Consumable exactly-once delivery | **Not shipped.** A crash after the exclusive claim but before actuation can leave the customer paid but undelivered; there is no actuator recovery journal or delivery sensor. |

## What you can verify now

From the repository root, using the pinned feature-enabled host:

```bash
./scripts/install-pinned-zeroclaw.sh
export PATH="$PWD/.build/zeroclaw-install/bin:$PATH"
./scripts/host-smoke.sh

zeroclaw config set sop.sops_dir "$PWD/sops"
zeroclaw sop validate proofkiosk-payment-loop
zeroclaw sop validate proofkiosk-sensor-loop
zeroclaw sop validate proofkiosk-heartbeat
zeroclaw sop graph proofkiosk-payment-loop
```

`validate` proves syntax and schema compatibility only. `host-smoke.sh` installs/loads
all components and uses deterministic local JSON-RPC fixtures to execute valid charge,
paid-watch, and unsigned-attest business paths through the exact pinned runtime. It also
passes the real host-direct charge and paid-watch results through immutable order
validation, one exclusive claim, and duplicate-claim rejection in an isolated directory.
It does not supply the external hardware/signer driver or prove a
public-Devnet host-direct run.

For the real boundaries of the payment example, read
[`payment-loop/SOP.md`](payment-loop/SOP.md) before filming or connecting a load.
