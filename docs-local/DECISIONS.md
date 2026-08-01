# Design decisions

Locked design choices, newest first. Each entry records what was chosen, what was
rejected, and what the rejected option would have broken — so a later reader does
not re-litigate a settled trade-off without the reasoning that settled it.

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
