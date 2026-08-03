# ProofKiosk — security and third-party trust model

ProofKiosk's strongest claim is narrow: no plugin holds a private key or can spend
funds. That claim does not mean the complete kiosk is trusted, autonomous, or safe to
connect to hardware. The actuator, delivery sensor, attestation signer/submission loop,
and physical recovery state machine are not shipped.

## Key custody

- `kiosk-charge` builds a customer-signable Solana Pay request. The customer's wallet
  signs and sends the transfer.
- `kiosk-watch` has no key and builds no transaction. It reads an operator-configured
  Solana RPC endpoint.
- `kiosk-attest` builds an unsigned durable-nonce message containing only System
  `AdvanceNonceAccount` and Memo instructions. An external signer must validate, sign,
  submit, and finalize it.
- Merchant, customer, nonce-authority, and device keys remain outside ZeroClaw. Config
  contains public addresses only, except that an authenticated RPC URL may itself be a
  credential.

`T0` and `T1` are **custody tiers**, not complete blast-radius ratings. In particular,
`kiosk-watch` and `kiosk-attest` receive a generic `http_client` capability. A compromised
component still cannot move funds, but it could falsify a verdict or misuse outbound HTTP
within the host's policy. The exact pinned host does not provide a ProofKiosk-specific
RPC-origin/method allowlist.

## What ProofKiosk trusts

1. **The operator's Solana RPC endpoint.** Watch and attest believe the chain state it
   reports. Payment verdicts require `finalized`, validate the RPC envelope and
   transaction, and fail closed on malformed/non-2xx responses, but a malicious endpoint
   can still lie. Use a trusted node or independent quorum for valuable deployments.
2. **Plaintext operator config before import.** Merchant, mint, token decimals, catalog,
   device identity, nonce accounts, and policy windows must agree across plugin rows.
   Run `scripts/check-config.sh` before ZeroClaw encrypts secret-looking values.
3. **Raw host-direct tool results at actuation boundaries.** Feed
   `trusted-charge-handoff.mjs` and `trusted-order-claim.mjs` the exact WIT `ToolResult`
   captured by the host. Never feed either script model prose, a chat transcript, or JSON
   reconstructed by an LLM.
4. **The future external signer and actuator driver.** Neither is implemented here. A
   signer must independently decode the unsigned artifact and serialize one durable-nonce
   artifact through finalization. A driver must validate trusted order state, claim it,
   and enforce fixed hardware bounds without consulting model prose.

## Code-backed invariants

- Charge/watch merchant, mint, configured decimals, and catalog prices are operator
  config and cannot be supplied through the model-facing schema.
- An actuation-eligible payment must be one successful, finalized, exact
  `transferChecked` transaction for the configured mint, decimals, amount, recipient,
  reference, item, and `PKPAY1` memo. The reference is a read-only non-signer, the transfer
  authority is an actual signer, and aggregate merchant balance delta must match exactly.
- Token decimals are explicit operator config and are checked against the transaction;
  the plugin does **not** query the mint account to discover them. A wrong value fails
  closed but can make every real payment look invalid.
- An authenticated `PKFUL1` marker counts only after its payment signature is reverified
  and its configured authority, device, reference, item, and instruction shape match.
  Recovery also requires the memo reference to occur exactly once as a read-only
  non-signer account, so an undiscoverable fulfillment memo cannot become a chain link.
- Authentication happens before version/schema interpretation. Outsider future-version
  memos remain ignorable public junk; an authority-signed future/malformed marker or
  attestation fails closed instead of being mistaken for an older safe schema.
- Inside the bounded attestation proof horizon, each `prev` must name the immediate
  authenticated predecessor and sequence zero is valid only directly above authenticated
  nonce initialization. Every authenticated v1 link must match one exact emitted reading,
  event, or fulfillment schema. Initialization is provisional until the remaining
  non-truncated scan proves no older authenticated incarnation is visible. A visible
  authorized reset or skipped predecessor is a chain gap; older history beyond the
  ten-authenticated-link checkpoint remains a documented bound.
- Heartbeat freshness uses the authenticated memo `ts`, not transaction landing time, so
  delayed durable-nonce submission cannot revive stale liveness. Exact nonce
  initialization terminates the scan as a new device-incarnation boundary.
- `kiosk-attest` cannot express a transfer: its message contains only Memo and System
  programs and carries zero signatures.
- Untrusted base58/base64, RPC JSON, account data, and memo data return errors rather than
  panic or silently become success.
- The built `kiosk-charge` component imports zero `wasi:http`, checked by
  `scripts/verify-no-network.sh`.

Business-negative outcomes such as `pending`, `mismatch`, `stale`, and
`missing` are versioned JSON objects in a successful WIT call with inner
`success:false`. Configuration, RPC, decode, and schema failures are different: the WIT
`ToolResult` has outer `success:false`, empty `output`, and a populated `error`. Drivers
must fail closed for both forms and must never infer authorization from the human summary.

## Host-local order safety

The trusted charge handoff validates the raw host-direct charge result against charge and
watch config, validates the exact Solana Pay URI and memo, and durably creates a mode-0600
order record containing immutable economics and quote expiry. The claim helper requires a
raw host-direct paid result to match reference, item, amount, recipient, mint, decimals,
and quote-window policy, and requires its verified block time to fall inside that quote.
It then creates one exclusive claim file; a second claim fails. A catalog change therefore
cannot silently underpay an older order, while delayed observation of an on-time payment
can recover after an outage.

This is a single-host **at-most-once claim primitive**, not exactly-once physical
delivery. A crash after the claim and before actuation can leave the customer paid but
undelivered. There is no claimed → actuating → delivered journal, delivery sensor,
automatic recovery policy, or relay implementation.

## Residual risks

- The public reference scan is bounded to the newest ten signatures. More than ten newer
  writes can hide a payment or authenticated marker. A hidden first payment fails closed;
  a hidden marker can make the plugin report `paid` again, so every actuator driver must
  also use the durable local claim.
- Attestation recovery scans at most 100 public device-address signatures and validates a
  ten-authenticated-link suffix (or reaches nonce initialization). Roughly 91 or more
  public/failed writes can crowd that proof out and halt new attestation construction;
  worst-case recovery can issue up to 100 sequential transaction lookups.
- The HTTP client has a connect timeout and a two-MiB body cap, but the selected client
  exposes no complete response/read deadline. A peer that connects and stalls can hold a
  call until the host/runtime limit intervenes.
- One durable nonce supports one pending artifact at a time. Build → sign → submit →
  finalize must complete before building the next artifact from that account.
- The attestation timestamp comes from the host OS clock, not secure hardware time.
- The attestation chain proves authenticated ordering, not content-hash consensus; an
  authorized signer can branch history.
- Physical enclosure bypass, relay polarity, boot state, motor safety, and sensor
  provenance are outside the shipped code.

## Reporting

For security concerns, open an issue on this repository without including credentials,
private keys, or real payment data.
