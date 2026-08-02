//! Host tests for the kiosk-watch core, driven exactly as the wasm shim drives
//! it: config from a flat section, strict args, RPC mocked via a one-method
//! transport. Plain `cargo test` — NO live network. Every fail-closed behavior
//! (and above all "RPC failure is NEVER Paid") is a test.

use std::cell::Cell;
use std::collections::HashMap;

use kiosk_core::rpc::{RpcError, RpcTransport};
use kiosk_watch::watch::{
    verify_heartbeat, verify_payment, Heartbeat, Verdict, WatchArgs, WatchConfig, WatchError,
    DEFAULT_USDC_MINT, FULFILLMENT_TAG,
};

// Valid 32-byte base58 pubkeys reused across cases.
const MERCHANT: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
const REFERENCE: &str = "11111111111111111111111111111111";
const DEVICE: &str = "So11111111111111111111111111111111111111112";
const OTHER_MINT: &str = "So11111111111111111111111111111111111111112";
const OTHER_OWNER: &str = "So11111111111111111111111111111111111111112";
/// The operator's device authority — the ONLY signer whose fulfillment marker
/// counts. Must equal kiosk-attest's `nonce_authority` in a real deployment.
const AUTHORITY: &str = "Vote111111111111111111111111111111111111111";
/// Anyone else. A marker they sign must be ignored, or they could block
/// delivery of a charge they never paid for.
const ATTACKER: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
const RPC: &str = "https://api.devnet.solana.com";
const NOW: u64 = 1_000_000;

// ── Mock transport: dispatch by RPC method, return canned bodies or an error ──

struct Mock {
    sig: Result<String, RpcError>,
    tx: Result<String, RpcError>,
    /// signature -> raw getTransaction `result`, for cases where the sig list
    /// holds several transactions that must resolve differently.
    tx_by_sig: HashMap<String, String>,
    sig_calls: Cell<u32>,
    tx_calls: Cell<u32>,
}

impl Mock {
    fn new(sig: Result<String, RpcError>, tx: Result<String, RpcError>) -> Self {
        Self {
            sig,
            tx,
            tx_by_sig: HashMap::new(),
            sig_calls: Cell::new(0),
            tx_calls: Cell::new(0),
        }
    }
    fn sigs(body: &str) -> Self {
        Mock::new(Ok(wrap(body)), Ok(wrap("null")))
    }
    fn full(sig_body: &str, tx_body: &str) -> Self {
        Mock::new(Ok(wrap(sig_body)), Ok(wrap(tx_body)))
    }
    /// Sig list plus a per-signature getTransaction table.
    fn routed(sig_body: &str, txs: &[(&str, String)]) -> Self {
        let mut m = Mock::new(Ok(wrap(sig_body)), Ok(wrap("null")));
        m.tx_by_sig = txs
            .iter()
            .map(|(s, body)| (s.to_string(), body.clone()))
            .collect();
        m
    }
}

impl RpcTransport for Mock {
    fn send(&self, request_body: &str) -> Result<String, RpcError> {
        if request_body.contains("getSignaturesForAddress") {
            self.sig_calls.set(self.sig_calls.get() + 1);
            self.sig.clone()
        } else if request_body.contains("getTransaction") {
            self.tx_calls.set(self.tx_calls.get() + 1);
            for (sig, body) in &self.tx_by_sig {
                if request_body.contains(sig.as_str()) {
                    return Ok(wrap(body));
                }
            }
            self.tx.clone()
        } else {
            Err(RpcError::Transport("unexpected method".into()))
        }
    }
}

/// Wrap a bare `result` value in a JSON-RPC envelope, as the node would.
fn wrap(result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result_json}}}"#)
}

/// The operator price list, identical in shape to the one `kiosk-charge` parses.
/// It is the ONLY source of the amount the relay gates on (Fix A).
const PRICE_LIST: &str = "cold_drink:1.5, day_pass:5";

fn cfg() -> WatchConfig {
    WatchConfig::from_section(
        &[
            ("rpc_url", RPC),
            ("merchant_address", MERCHANT),
            ("price_list", PRICE_LIST),
            ("device_authority", AUTHORITY),
            // usdc_mint defaults to mainnet USDC; finality defaults to "confirmed"
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    )
    .unwrap()
}

fn pay_args() -> WatchArgs {
    WatchArgs {
        reference: Some(REFERENCE.into()),
        item_id: Some("cold_drink".into()),
        window_s: Some(300),
        ..Default::default()
    }
}

/// Payment args for an arbitrary catalog item.
fn args_for(item: &str) -> WatchArgs {
    WatchArgs {
        item_id: Some(item.into()),
        ..pay_args()
    }
}

// One signature referencing the charge, `age` seconds before NOW.
fn one_sig(age: u64) -> String {
    let bt = NOW - age;
    format!(
        r#"[{{"signature":"5xSig","slot":100,"err":null,"blockTime":{bt},"confirmationStatus":"confirmed"}}]"#
    )
}

// A getTransaction result crediting `amount` base units of `mint` to `owner`,
// with the reference present in accountKeys and meta.err = `err`.
fn tx(owner: &str, mint: &str, amount: &str, err: &str) -> String {
    format!(
        r#"{{
          "slot":100,"blockTime":{bt},
          "meta":{{"err":{err},
            "preTokenBalances":[{{"accountIndex":3,"mint":"{mint}","owner":"{owner}","uiTokenAmount":{{"amount":"0","decimals":6}}}}],
            "postTokenBalances":[{{"accountIndex":3,"mint":"{mint}","owner":"{owner}","uiTokenAmount":{{"amount":"{amount}","decimals":6}}}}]}},
          "transaction":{{"message":{{"accountKeys":[
            {{"pubkey":"PayerAcct"}},{{"pubkey":"MerchantAta"}},{{"pubkey":"TokenProgram"}},{{"pubkey":"{reference}"}}]}}}}
        }}"#,
        bt = NOW - 5,
        reference = REFERENCE,
    )
}

// A getTransaction result whose accountKeys do NOT include the queried
// reference — a payment for a *different* charge (replay attempt).
fn tx_without_reference(owner: &str, mint: &str, amount: &str) -> String {
    format!(
        r#"{{
          "slot":100,"blockTime":{bt},
          "meta":{{"err":null,
            "preTokenBalances":[{{"accountIndex":3,"mint":"{mint}","owner":"{owner}","uiTokenAmount":{{"amount":"0","decimals":6}}}}],
            "postTokenBalances":[{{"accountIndex":3,"mint":"{mint}","owner":"{owner}","uiTokenAmount":{{"amount":"{amount}","decimals":6}}}}]}},
          "transaction":{{"message":{{"accountKeys":[
            {{"pubkey":"PayerAcct"}},{{"pubkey":"MerchantAta"}},{{"pubkey":"TokenProgram"}}]}}}}
        }}"#,
        bt = NOW - 5,
    )
}

// ── Fix B fixtures: the fulfillment marker ───────────────────────────────────

/// A signature-list entry for a fulfillment marker: a memo tx carrying the
/// PKFUL1 tag and naming this charge.
fn marker_sig_entry(signature: &str, age: u64) -> String {
    let bt = NOW - age;
    let memo = format!(
        r#"[90] {{\"v\":1,\"dev\":\"k01\",\"seq\":3,\"tag\":\"{FULFILLMENT_TAG}\",\"ref\":\"{REFERENCE}\"}}"#
    );
    format!(
        r#"{{"signature":"{signature}","slot":101,"err":null,"blockTime":{bt},"memo":"{memo}","confirmationStatus":"confirmed"}}"#
    )
}

/// A plain (non-marker) signature-list entry.
fn plain_sig_entry(signature: &str, age: u64) -> String {
    let bt = NOW - age;
    format!(
        r#"{{"signature":"{signature}","slot":100,"err":null,"blockTime":{bt},"memo":null,"confirmationStatus":"confirmed"}}"#
    )
}

fn sig_list(entries: &[String]) -> String {
    format!("[{}]", entries.join(","))
}

/// A getTransaction result for a marker: memo-only, signed by `signer`, with
/// the charge reference present as a read-only key.
fn marker_tx(signer: &str) -> String {
    format!(
        r#"{{
          "slot":101,"blockTime":{bt},
          "meta":{{"err":null,"preTokenBalances":[],"postTokenBalances":[]}},
          "transaction":{{"message":{{"accountKeys":[
            {{"pubkey":"{signer}","signer":true,"writable":true}},
            {{"pubkey":"NonceAcct","signer":false,"writable":true}},
            {{"pubkey":"{reference}","signer":false,"writable":false}}]}}}}
        }}"#,
        bt = NOW - 3,
        reference = REFERENCE,
    )
}

/// A transaction that references this charge but moves no tokens at all — the
/// junk any stranger can write to a public reference pubkey.
fn junk_tx() -> String {
    format!(
        r#"{{
          "slot":99,"blockTime":{bt},
          "meta":{{"err":null,"preTokenBalances":[],"postTokenBalances":[]}},
          "transaction":{{"message":{{"accountKeys":[
            {{"pubkey":"{ATTACKER}","signer":true,"writable":true}},
            {{"pubkey":"{reference}","signer":false,"writable":false}}]}}}}
        }}"#,
        bt = NOW - 2,
        reference = REFERENCE,
    )
}

fn cfg_with_mint(mint: &str) -> WatchConfig {
    WatchConfig::from_section(
        &[
            ("rpc_url", RPC),
            ("merchant_address", MERCHANT),
            ("usdc_mint", mint),
            ("price_list", PRICE_LIST),
            ("device_authority", AUTHORITY),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    )
    .unwrap()
}

// ── USER-FRIENDLY + SECURE: human errors that leak no secrets ────────────────

#[test]
fn misconfig_errors_are_human_and_leak_no_rpc_url() {
    let sec = |pairs: &[(&str, &str)]| -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    // Missing rpc_url → names the missing key.
    let e = WatchConfig::from_section(&sec(&[("merchant_address", MERCHANT)])).unwrap_err();
    assert!(e.to_string().contains("rpc_url"), "unhelpful: {e}");
    // Invalid merchant → names the field and says what's wrong.
    let e2 = WatchConfig::from_section(&sec(&[("rpc_url", RPC), ("merchant_address", "xx")]))
        .unwrap_err();
    let s = e2.to_string();
    assert!(
        s.contains("merchant_address") && s.contains("pubkey"),
        "unhelpful: {s}"
    );
    // The configured RPC endpoint never appears in an error message.
    assert!(!s.contains(RPC), "rpc_url leaked into error: {s}");
}

// ── FAST: bounded RPC — one getSignaturesForAddress + at most one getTransaction ──

#[test]
fn paid_path_makes_exactly_one_sig_and_one_tx_call() {
    let mock = Mock::full(
        &one_sig(5),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
    );
    // Borrow (via impl RpcTransport for &T) so we can read the counters after.
    let v = verify_payment(&pay_args(), &cfg(), &mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Paid { .. }));
    assert_eq!(
        mock.sig_calls.get(),
        1,
        "exactly one getSignaturesForAddress"
    );
    assert_eq!(mock.tx_calls.get(), 1, "at most one getTransaction");
}

#[test]
fn pending_path_makes_one_sig_and_zero_tx_calls() {
    let mock = Mock::sigs("[]");
    let v = verify_payment(&pay_args(), &cfg(), &mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Pending));
    assert_eq!(mock.sig_calls.get(), 1);
    assert_eq!(
        mock.tx_calls.get(),
        0,
        "no getTransaction when nothing to verify"
    );
}

// ── SECURE: replay / double-spend — a tx for a different charge cannot clear this one ──

#[test]
fn payment_not_referencing_this_charge_is_mismatch() {
    // The reference is single-use: a landed payment whose tx does not reference
    // THIS charge must never verify it (prevents replaying one payment across sales).
    let mock = Mock::full(
        &one_sig(5),
        &tx_without_reference(MERCHANT, DEFAULT_USDC_MINT, "1500000"),
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

// ── BROADLY-USABLE: any SPL mint, not just USDC ──────────────────────────────

#[test]
fn verifies_a_non_usdc_spl_mint() {
    // Operator configures a different stablecoin/token mint; verification generalizes.
    let mint = "So11111111111111111111111111111111111111112";
    let mock = Mock::full(&one_sig(5), &tx(MERCHANT, mint, "1500000", "null"));
    let v = verify_payment(&pay_args(), &cfg_with_mint(mint), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Paid { .. }), "got {v:?}");
}

// ── Payment: happy path ──────────────────────────────────────────────────────

#[test]
fn paid_happy_path() {
    let mock = Mock::full(
        &one_sig(5),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    match v {
        Verdict::Paid {
            payer,
            signature,
            slot,
        } => {
            assert_eq!(payer, "PayerAcct");
            assert_eq!(signature, "5xSig");
            assert_eq!(slot, 100);
        }
        other => panic!("expected Paid, got {other:?}"),
    }
}

#[test]
fn pending_when_no_signatures() {
    let mock = Mock::sigs("[]");
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Pending), "got {v:?}");
}

#[test]
fn wrong_amount_is_mismatch() {
    let mock = Mock::full(
        &one_sig(5),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "500000", "null"),
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

#[test]
fn wrong_recipient_is_mismatch() {
    let mock = Mock::full(
        &one_sig(5),
        &tx(OTHER_OWNER, DEFAULT_USDC_MINT, "1500000", "null"),
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

#[test]
fn wrong_mint_is_mismatch() {
    let mock = Mock::full(&one_sig(5), &tx(MERCHANT, OTHER_MINT, "1500000", "null"));
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

#[test]
fn stale_payment_outside_window_is_expired() {
    // A matching signature exists but landed 1 hour before now; window is 60s.
    let mut args = pay_args();
    args.window_s = Some(60);
    let mock = Mock::full(
        &one_sig(3600),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
    );
    let v = verify_payment(&args, &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Expired), "got {v:?}");
}

#[test]
fn on_chain_failed_tx_is_mismatch_not_paid() {
    let mock = Mock::full(
        &one_sig(5),
        &tx(
            MERCHANT,
            DEFAULT_USDC_MINT,
            "1500000",
            r#"{"InstructionError":[0,"Custom"]}"#,
        ),
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

// ── Payment: fail-closed — RPC / decode failures are NEVER Paid ──────────────

#[test]
fn rpc_error_is_err_never_paid() {
    let mock = Mock::new(
        Ok(wrap(&one_sig(5))),
        Err(RpcError::Transport("boom".into())),
    );
    let r = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(matches!(r, Err(WatchError::Rpc(_))), "got {r:?}");
}

#[test]
fn signatures_rpc_error_is_err_never_paid() {
    let mock = Mock::new(
        Err(RpcError::Rpc {
            code: -32000,
            message: "node down".into(),
        }),
        Ok(wrap("null")),
    );
    let r = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(r.is_err(), "RPC failure must never be Paid; got {r:?}");
    assert!(!matches!(r, Ok(Verdict::Paid { .. })));
}

#[test]
fn malformed_get_transaction_is_err_never_paid() {
    // Signature exists, but getTransaction lacks meta/accountKeys entirely.
    let mock = Mock::full(&one_sig(5), r#"{"foo":1}"#);
    let r = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(matches!(r, Err(WatchError::Decode(_))), "got {r:?}");
}

// ── Args / config fail-closed ────────────────────────────────────────────────

#[test]
fn deny_unknown_fields_rejects_smuggled_key() {
    let raw = r#"{"reference":"x","item_id":"cold_drink","rpc_url":"http://evil"}"#;
    let parsed: Result<WatchArgs, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "smuggled `rpc_url` must fail deserialization"
    );
}

// ── Fix A: the gating amount comes from operator config, never the model ─────

#[test]
fn watch_rejects_model_supplied_amount() {
    // `expected_amount` is no longer a field at all. Under deny_unknown_fields
    // that makes "charge them 0.001 instead" a hard deserialization failure,
    // before any verification logic runs.
    let raw = r#"{"reference":"11111111111111111111111111111111","expected_amount":"0.001"}"#;
    let parsed: Result<WatchArgs, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "a model-supplied amount must not deserialize"
    );
}

#[test]
fn exact_item_price_is_paid() {
    // config prices day_pass at 5 USDC; the tx credits exactly 5_000_000 base units.
    let mock = Mock::full(
        &one_sig(5),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "5000000", "null"),
    );
    let v = verify_payment(&args_for("day_pass"), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Paid { .. }), "got {v:?}");
}

#[test]
fn underpay_for_item_is_mismatch() {
    // config prices day_pass at 5; a 1 USDC payment must not clear it.
    let mock = Mock::full(
        &one_sig(5),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "1000000", "null"),
    );
    let v = verify_payment(&args_for("day_pass"), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

#[test]
fn unknown_item_id_is_args_error() {
    // The price list is the allowlist: an item that is not in it has no price,
    // so there is nothing to verify against. Fail closed, before any RPC.
    let mock = Mock::sigs("[]");
    let r = verify_payment(&args_for("free_everything"), &cfg(), &mock, NOW);
    match r {
        Err(WatchError::Args(m)) => assert!(
            m.contains("free_everything"),
            "error should name the rejected item: {m}"
        ),
        other => panic!("expected Args error, got {other:?}"),
    }
    assert_eq!(mock.sig_calls.get(), 0, "must fail before touching the RPC");
}

#[test]
fn missing_item_id_is_args_error() {
    // A free-amount charge (kiosk_charge amount_usdc) carries no item_id. Those
    // are invoicing-only and never actuation-eligible — and the error says so,
    // rather than reading like a missing-argument bug.
    let args = WatchArgs {
        reference: Some(REFERENCE.into()),
        ..Default::default()
    };
    let mock = Mock::sigs("[]");
    let r = verify_payment(&args, &cfg(), &mock, NOW);
    match r {
        Err(WatchError::Args(m)) => {
            assert!(m.contains("item_id"), "error should name item_id: {m}");
            assert!(
                m.contains("free-amount"),
                "error must name the invoicing-only class: {m}"
            );
        }
        other => panic!("expected Args error, got {other:?}"),
    }
    assert_eq!(mock.sig_calls.get(), 0, "must fail before touching the RPC");
}

// ── Fix B: single-use actuation via an authenticated on-chain marker ─────────

#[test]
fn no_marker_valid_payment_is_paid() {
    // Baseline: nothing has been fulfilled, the payment is good → Paid.
    let mock = Mock::routed(
        &sig_list(&[plain_sig_entry("5xSig", 5)]),
        &[("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"))],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Paid { .. }), "got {v:?}");
}

#[test]
fn authenticated_fulfillment_marker_is_already_fulfilled() {
    // The payment is on-chain AND a PKFUL1 marker signed by the device
    // authority exists → this charge was already delivered. Do not re-fire.
    let mock = Mock::routed(
        &sig_list(&[
            marker_sig_entry("MarkerSig", 2),
            plain_sig_entry("5xSig", 5),
        ]),
        &[
            ("MarkerSig", marker_tx(AUTHORITY)),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert_eq!(v, Verdict::AlreadyFulfilled, "got {v:?}");
    assert!(!v.is_paid(), "AlreadyFulfilled must never gate the relay");
}

#[test]
fn spoofed_fulfillment_wrong_signer_is_ignored() {
    // DoS defense: anyone can write a PKFUL1 memo naming a public reference.
    // Only the device authority's marker counts, so a stranger cannot block
    // delivery of a charge that was genuinely paid.
    let mock = Mock::routed(
        &sig_list(&[marker_sig_entry("SpoofSig", 2), plain_sig_entry("5xSig", 5)]),
        &[
            ("SpoofSig", marker_tx(ATTACKER)),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(v, Verdict::Paid { .. }),
        "a spoofed marker must not block a real payment; got {v:?}"
    );
}

#[test]
fn replay_after_fulfillment() {
    // Tick 1: paid, nothing fulfilled → relay fires.
    let before = Mock::routed(
        &sig_list(&[plain_sig_entry("5xSig", 5)]),
        &[("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"))],
    );
    let first = verify_payment(&pay_args(), &cfg(), before, NOW).unwrap();
    assert!(matches!(first, Verdict::Paid { .. }), "got {first:?}");

    // Tick 2: the marker has landed. The SAME payment must not clear again.
    let after = Mock::routed(
        &sig_list(&[
            marker_sig_entry("MarkerSig", 1),
            plain_sig_entry("5xSig", 5),
        ]),
        &[
            ("MarkerSig", marker_tx(AUTHORITY)),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );
    let second = verify_payment(&pay_args(), &cfg(), after, NOW).unwrap();
    assert_eq!(second, Verdict::AlreadyFulfilled, "got {second:?}");
}

#[test]
fn junk_tx_after_payment_still_verifies() {
    // The reference is public (it is in the QR the customer scans), so anyone
    // can write a transaction naming it. A junk tx landing AFTER the real
    // payment must not hide it — otherwise a stranger blocks every sale for
    // the price of one memo.
    let mock = Mock::routed(
        &sig_list(&[plain_sig_entry("JunkSig", 2), plain_sig_entry("5xSig", 5)]),
        &[
            ("JunkSig", junk_tx()),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(v, Verdict::Paid { .. }),
        "a junk tx must not mask the real payment; got {v:?}"
    );
}

#[test]
fn missing_device_authority_fails_closed() {
    // Without it no marker can be authenticated, so single-use cannot be
    // enforced. Refuse to verify rather than actuate with the check disabled.
    let section = [
        ("rpc_url", RPC),
        ("merchant_address", MERCHANT),
        ("price_list", PRICE_LIST),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let cfg = WatchConfig::from_section(&section).unwrap();
    let mock = Mock::sigs("[]");
    let r = verify_payment(&pay_args(), &cfg, &mock, NOW);
    assert!(matches!(r, Err(WatchError::Config(_))), "got {r:?}");
    assert_eq!(mock.sig_calls.get(), 0, "must fail before touching the RPC");
}

#[test]
fn already_fulfilled_summary_within_token_budget() {
    let v = Verdict::AlreadyFulfilled;
    assert!(
        kiosk_core::shape::approx_tokens(&v.summary()) <= kiosk_core::shape::DEFAULT_BUDGET_TOKENS
    );
}

#[test]
fn missing_rpc_url_fails_closed() {
    let section = [("merchant_address", MERCHANT)]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let err = WatchConfig::from_section(&section).unwrap_err();
    assert!(matches!(err, WatchError::Config(_)), "got {err:?}");
}

#[test]
fn missing_reference_arg_fails_closed() {
    let args = WatchArgs {
        item_id: Some("cold_drink".into()),
        ..Default::default()
    };
    let mock = Mock::sigs("[]");
    let r = verify_payment(&args, &cfg(), mock, NOW);
    assert!(matches!(r, Err(WatchError::Args(_))), "got {r:?}");
}

#[test]
fn summary_within_token_budget() {
    let mock = Mock::full(
        &one_sig(5),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        kiosk_core::shape::approx_tokens(&v.summary()) <= kiosk_core::shape::DEFAULT_BUDGET_TOKENS
    );
}

// ── Heartbeat mode ───────────────────────────────────────────────────────────

fn hb_args(max_silence_s: u64) -> WatchArgs {
    WatchArgs {
        mode: Some("heartbeat".into()),
        device_address: Some(DEVICE.into()),
        max_silence_s: Some(max_silence_s),
        ..Default::default()
    }
}

#[test]
fn heartbeat_live_when_recent() {
    let mock = Mock::sigs(&one_sig(30));
    let h = verify_heartbeat(&hb_args(300), &cfg(), mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Live { .. }), "got {h:?}");
}

#[test]
fn heartbeat_stale_when_old() {
    let mock = Mock::sigs(&one_sig(3600));
    let h = verify_heartbeat(&hb_args(300), &cfg(), mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Stale { .. }), "got {h:?}");
}

#[test]
fn heartbeat_missing_when_no_signatures() {
    let mock = Mock::sigs("[]");
    let h = verify_heartbeat(&hb_args(300), &cfg(), mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Missing), "got {h:?}");
}

#[test]
fn heartbeat_rpc_error_is_err_never_live() {
    let mock = Mock::new(Err(RpcError::Transport("boom".into())), Ok(wrap("null")));
    let r = verify_heartbeat(&hb_args(300), &cfg(), mock, NOW);
    assert!(r.is_err());
    assert!(!matches!(r, Ok(Heartbeat::Live { .. })));
}

#[test]
fn heartbeat_missing_device_address_fails_closed() {
    let args = WatchArgs {
        mode: Some("heartbeat".into()),
        max_silence_s: Some(300),
        ..Default::default()
    };
    let mock = Mock::sigs("[]");
    let r = verify_heartbeat(&args, &cfg(), mock, NOW);
    assert!(matches!(r, Err(WatchError::Args(_))), "got {r:?}");
}
