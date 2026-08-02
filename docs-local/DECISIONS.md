# Design decisions

Locked design choices, newest first. Each entry records what was chosen, what was
rejected, and what the rejected option would have broken — so a later reader does
not re-litigate a settled trade-off without the reasoning that settled it.

---

## 2026-08-02 — Phase 0: status banner separating shipped from roadmap (audit #8)

**Locked: a status table at the very top of the README, above the architecture, plus
inline scope notes on every claim below it that outruns the code.** No claim was deleted;
unproven ones moved under an explicit roadmap marker.

**Why this wins.** The README's opening sentence asserted the agent "delivers it only
after the payment is verified on-chain, and writes tamper-evident proof." Both are design
intent, neither runs end to end: the SOP guard predicate does not resolve, so no relay
has ever been pulsed by this code, and no attestation has been signed or landed. A reader
who builds on that sentence discovers the gap after they trust it. A judge who finds it
discounts everything else in the repo, including the parts that are real and tested — the
cost of one unproven claim is the credibility of the 127 that hold.

**What the audit asked for vs what shipped — one deliberate deviation.** The brief's
"runs today" list included `finalized`. It is not true today: `WatchConfig` defaults
`finality` to `confirmed` and still accepts `processed`. Requiring `finalized` for an
actuating verdict is audit #22, scheduled for Phase 1. Writing "finalized" into the table
now would have put a false claim inside the artifact whose entire purpose is removing
false claims, so the row is listed as roadmap with the current default named explicitly.
Phase 1 flips it.

**Rejected — a short "status: WIP" note instead of a per-capability table.** A blanket
disclaimer is unfalsifiable and reads as boilerplate; it protects the author without
informing the reader. A per-row table forces each claim to name its evidence or its gap,
which is also what makes it maintainable: when Phase 1 lands `finalized`, exactly one row
changes.

**Rejected — deleting the aspirational framing.** The vision is the point of the project
and the roadmap is legitimate. The problem was never ambition; it was ambition written in
the present tense.

**Also corrected while verifying:** the component sizes in three READMEs were stale
(watch 348→356 KB, attest 384→389 KB) because Fix A/B added code. Caught by measuring
rather than trusting the existing text — the same failure mode the phase is about.

**Reusable takeaway:** put the falsifiable claim next to the thing that falsifies it. A
status row that cites a test count or names a missing predicate cannot rot quietly; a
paragraph of prose can.

---

## 2026-08-02 — Fix B: single-use delivery via an authenticated on-chain marker

**Locked: option (a) — an on-chain fulfillment memo authenticated by the device
authority.** After actuation, `kiosk-attest` builds a `PKFUL1` marker naming the charge;
`kiosk-watch` scans the reference for one and returns `AlreadyFulfilled` (never `Paid`)
when it finds one it can authenticate.

**Why this wins.** A verified payment stays verified forever, so a stateless verifier
polled on a cron will re-authorize the same charge on every tick. The component *is*
stateless by construction — the host builds a fresh WASI store and fuel budget per
`execute`, so a counter silently resets — which means "already delivered" cannot be
remembered. It has to be a fact about the world, and the chain is the one piece of shared
state both the kiosk and an auditor can read.

**Authentication is the whole design, not a hardening pass.** The reference is public —
it is printed in the QR the customer scans — so anyone can write a memo naming it.
A marker believed on sight would hand every passer-by a veto over deliveries: write
`PKFUL1` at a charge and it can never be fulfilled. So a marker counts only if it
succeeded on-chain, names this charge, **and** was signed by the operator's
`device_authority`. That last condition is unforgeable, and it fails *open* on purpose —
an unauthenticated marker is treated as not-a-marker, because a fake must never withhold
a delivery someone paid for.

**Rejected — (b) a marker account / PDA.** Strictly better semantics (a real
"exists / doesn't exist" bit rather than a memo scan) and it would sidestep the
scan-window limit below. It needs an on-chain program: something to write, deploy,
upgrade, audit, and trust. That is a large new trust surface for a repo whose central
claim is how little it asks you to trust, and it would put a deployed program between
the kiosk and its own safety property. Worth revisiting if ProofKiosk ever ships its own
program for other reasons.

**Rejected — (c) SOP one-shot only.** The SOP cannot enforce it: the cron trigger starts
a *new* run each tick, and `max_concurrent`/`admission_policy` bound concurrency, not
identity. There is nothing keyed to the reference for a one-shot to be one-shot *about*.
It would read as a guarantee while providing none — the worst kind of safety mechanism.

**Ordering: relay → marker (at-least-once), decided against marker-first.** Neither
ordering is atomic, because the marker is an unsigned transaction an external signer
submits out of band. The choice is therefore which failure to prefer:

| Ordering | Failure mode | Who is hurt |
|---|---|---|
| relay → marker (**chosen**) | marker write fails → a later tick re-fires | nobody, for an idempotent actuator |
| marker → relay | relay fails after the marker lands → charge reads fulfilled, nothing delivered | **the customer who paid** |

For a lock, a gate, or a charger enable, a re-fire is a harmless re-unlock. Robbing a
paying customer is not harmless. **This inverts for a consumable dispenser**, where a
second drink is a real loss — those need at-most-once ordering plus an operator retry
path, which is a config policy this repo does not ship. Documented as a deployment
constraint in `sops/payment-loop/SOP.md` and `docs/threat-model.md` rather than hidden
behind a flag.

**Adopted along the way.** Two findings surfaced while grilling this design:

1. **`device_authority` must equal `kiosk-attest`'s `nonce_authority`** — that is the fee
   payer and only required signer of every marker. Two separate config sections that
   cannot read each other, and a mismatch disables single-use *silently*. Hence
   `scripts/check-config.sh`, which also catches a drifted `price_list`.
2. **A pre-existing DoS.** `verify_payment` inspected only the newest signature on the
   reference, so one junk transaction written after a real payment masked it and the
   charge read as `Mismatch` forever — anyone could block every sale for the price of a
   memo. Fixed by scanning the list newest-first; the first transaction that fully
   verifies wins.

**Residual, stated rather than solved:** the scan reads the newest 10 signatures, so an
attacker who writes more than that to a reference can push the payment or its marker out
of view. The payment case fails closed; the marker case degrades to at-least-once.

**Implementation note.** The reference hangs off the `AdvanceNonceAccount` instruction,
not the memo: SPL Memo v2 rejects any account passed to it that is not a signer, and the
kiosk cannot sign for a reference keypair it does not hold. The System program reads only
accounts 0..=2 of that instruction, so an extra read-only key is inert on-chain while
still making the transaction discoverable — the same mechanism Solana Pay uses.

**Reusable takeaway:** when a component cannot hold state, put the state where an
adversary can also write — then make the *signature*, not the presence, of the record be
what counts. Verify-don't-trust turns shared public storage into private state.

---

## 2026-08-02 — Fix A: the gating amount is operator config, never the model

**Locked: option (a) — mirror `kiosk-charge` on the watch side.** `WatchConfig` parses
the same `price_list` key `ChargeConfig` already parses; `WatchArgs` drops
`expected_amount` and gains `item_id`; `verify_payment` derives the expected amount from
`cfg.price_list[item_id]`. `deny_unknown_fields` turns any surviving `expected_amount`
key into a hard deserialization error before a single line of verification logic runs.

**Why this wins.** The relay gates on `Verdict::Paid`, and `Paid` is reachable only when
`delta == expected_units`. Before this change `expected_units` came from a model-facing
argument, so the *number the relay gates on* was reachable from the prompt — the operator
cap in `kiosk-charge` bounded what a customer could be *asked* to pay, but nothing bounded
what `kiosk_watch` would *accept* as full payment. A charge for 5 USDC could be cleared by
telling watch to expect 0.001. Deriving the amount from config closes that by construction
rather than by validation: there is no argument left to lie in.

**Rejected — (b) a `min_amount_usdc` floor.** A floor bounds an amount from below; it does
not anchor it to a price. With a 1 USDC floor and a 5 USDC item, a 1 USDC payment still
clears. It only fits a weaker "pay at least" model (a donation box, a tip jar), which is
not what a kiosk selling a priced item is. Kept on record because that weaker model is a
real one — an operator building a pay-what-you-want kiosk should reach for (b), and should
know it is not what ships here.

**Rejected — (c) both.** (b) adds a second, weaker check underneath an exact-equality
check that already subsumes it. Dead configuration surface, one more key to get wrong.

**Consequence made explicit: charges now fall into two classes.** `kiosk-charge` still
accepts a free amount (`ChargeArgs.amount_usdc`, capped by `max_amount_usdc`) which
produces a charge with no `item_id`. After this change those charges are structurally
unverifiable — there is no price-list entry to look up — so:

- **Item-priced charges** (`item_id` from `price_list`) are **actuation-eligible**.
  `kiosk_watch` can verify them and the relay can fire.
- **Free-amount charges** (`amount_usdc`) are **invoicing-only** and are **never**
  actuation-eligible. `kiosk_watch` refuses them with a specific error naming the class,
  rather than a generic "missing argument" that reads like a bug.

This split is deliberate and is documented in `plugins/kiosk-charge/src/charge.rs`, both
plugin READMEs, and `docs/threat-model.md`. The alternative — deleting the free-amount
path — was not taken because invoicing a custom amount is a legitimate use of the charge
plugin on its own; it simply must not reach a relay.

**Reusable takeaway:** if a value gates a physical action, it must not be an argument.
Bound it in config and look it up by an opaque key the caller may choose from, so the
worst a compromised caller can do is pick the wrong row — never write the value.
