//! Host tests for the kiosk-watch core, driven exactly as the wasm shim drives
//! it: config from a flat section, strict args, RPC mocked via a one-method
//! transport. Plain `cargo test` — NO live network. Every fail-closed behavior
//! (and above all "RPC failure is NEVER Paid") is a test.

use std::cell::Cell;
use std::collections::HashMap;

use kiosk_core::rpc::{RpcError, RpcTransport};
use kiosk_watch::watch::{
    verify_heartbeat, verify_payment, Heartbeat, Verdict, WatchArgs, WatchConfig, WatchError,
    DEFAULT_USDC_MINT, FULFILLMENT_TAG, PAYMENT_TAG,
};

// Valid 32-byte base58 pubkeys reused across cases.
const MERCHANT: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
const REFERENCE: &str = "Stake11111111111111111111111111111111111111";
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
const PAYMENT_SIGNATURE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
const COMPUTE_BUDGET_PROGRAM: &str = "ComputeBudget111111111111111111111111111111";
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const RECENT_BLOCKHASHES_SYSVAR: &str = "SysvarRecentB1ockHashes11111111111111111111";
const RENT_SYSVAR: &str = "SysvarRent111111111111111111111111111111111";

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
            ("device_address", DEVICE),
            ("device_id", "kiosk01"),
            // usdc_mint defaults to mainnet USDC; finality defaults to "finalized"
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
    one_sig_for(age, "cold_drink")
}

fn one_sig_for(age: u64, item_id: &str) -> String {
    let bt = NOW - age;
    format!("[{}]", payment_sig_entry("5xSig", bt, REFERENCE, item_id))
}

fn payment_sig_entry(signature: &str, block_time: u64, reference: &str, item_id: &str) -> String {
    let claim = serde_json::json!({
        "v": 1,
        "tag": PAYMENT_TAG,
        "ref": reference,
        "item": item_id,
    })
    .to_string();
    serde_json::json!({
        "signature": signature,
        "slot": 100,
        "err": null,
        "blockTime": block_time,
        "memo": format!("[{}] {claim}", claim.len()),
        "confirmationStatus": "confirmed",
    })
    .to_string()
}

// A getTransaction result crediting `amount` base units of `mint` to `owner`,
// with the reference present in accountKeys and meta.err = `err`.
fn tx(owner: &str, mint: &str, amount: &str, err: &str) -> String {
    tx_for_item("cold_drink", owner, mint, amount, 6, err)
}

fn tx_for_item(
    item_id: &str,
    owner: &str,
    mint: &str,
    amount: &str,
    decimals: u8,
    err: &str,
) -> String {
    payment_tx_value(item_id, owner, mint, amount, decimals, err, true).to_string()
}

fn payment_tx_value(
    item_id: &str,
    owner: &str,
    mint: &str,
    amount: &str,
    decimals: u8,
    err: &str,
    include_reference: bool,
) -> serde_json::Value {
    let memo = serde_json::json!({
        "v": 1,
        "tag": PAYMENT_TAG,
        "ref": REFERENCE,
        "item": item_id,
    })
    .to_string();
    let mut account_keys = vec![
        serde_json::json!({"pubkey":"PayerAcct", "signer":true, "writable":true}),
        serde_json::json!({"pubkey":"MerchantAta", "signer":false, "writable":true}),
        serde_json::json!({"pubkey":"SourceAta", "signer":false, "writable":true}),
        serde_json::json!({"pubkey":mint, "signer":false, "writable":false}),
    ];
    if include_reference {
        account_keys.push(serde_json::json!({
            "pubkey": REFERENCE,
            "signer": false,
            "writable": false,
        }));
    }
    account_keys.extend([
        serde_json::json!({"pubkey":COMPUTE_BUDGET_PROGRAM,"signer":false,"writable":false}),
        serde_json::json!({"pubkey":MEMO_PROGRAM,"signer":false,"writable":false}),
        serde_json::json!({"pubkey":TOKEN_PROGRAM,"signer":false,"writable":false}),
    ]);
    serde_json::json!({
        "slot": 100,
        "blockTime": NOW - 5,
        "meta": {
            "err": serde_json::from_str::<serde_json::Value>(err).unwrap(),
            "preTokenBalances": [{
                "accountIndex": 1,
                "mint": mint,
                "owner": owner,
                "uiTokenAmount": {"amount":"0", "decimals":decimals}
            }],
            "postTokenBalances": [{
                "accountIndex": 1,
                "mint": mint,
                "owner": owner,
                "uiTokenAmount": {"amount":amount, "decimals":decimals}
            }]
        },
        "transaction": {"message": {
            "accountKeys": account_keys,
            "instructions": [
                {"programId":COMPUTE_BUDGET_PROGRAM, "accounts":[], "data":""},
                {"program":"spl-memo", "programId":MEMO_PROGRAM, "parsed":memo},
                {"program":"spl-token", "programId":TOKEN_PROGRAM, "parsed":{
                    "type":"transferChecked",
                    "info":{
                        "destination":"MerchantAta",
                        "mint":mint,
                        "multisigAuthority":"PayerAcct",
                        "signers":[REFERENCE],
                        "source":"SourceAta",
                        "tokenAmount":{"amount":amount,"decimals":decimals}
                    }
                }}
            ]
        }}
    })
}

// A getTransaction result whose accountKeys do NOT include the queried
// reference — a payment for a *different* charge (replay attempt).
fn tx_without_reference(owner: &str, mint: &str, amount: &str) -> String {
    payment_tx_value("cold_drink", owner, mint, amount, 6, "null", false).to_string()
}

// ── #42 / #43: balance-delta fixtures ────────────────────────────────────────

/// Credits `amount` to the merchant but omits the matching *pre* balance, so
/// the prior balance is unknown rather than zero.
fn tx_without_pre_balance(amount: &str) -> String {
    let mut tx = payment_tx_value(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        amount,
        6,
        "null",
        true,
    );
    tx["meta"]["preTokenBalances"] = serde_json::json!([]);
    tx.to_string()
}

/// A payment in a mint whose decimals are NOT 6.
fn tx_with_decimals(amount: &str, decimals: u8) -> String {
    tx_for_item(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        amount,
        decimals,
        "null",
    )
}

#[test]
fn missing_pre_balance_is_a_decode_error_not_a_zero() {
    // delta = post - pre. Treating an absent pre entry as zero makes delta the
    // WHOLE post balance, so an account that already held the price would
    // verify a payment that never happened. An unknown prior balance is
    // unknown, not zero.
    let mock = Mock::routed(
        &sig_list(&[plain_sig_entry("5xSig", 5)]),
        &[("5xSig", tx_without_pre_balance("1500000"))],
    );
    let r = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(r, Err(WatchError::Decode(_))),
        "an unknown prior balance must fail closed, got {r:?}"
    );
}

#[test]
fn amount_is_scaled_by_the_mints_real_decimals() {
    // USDC_DECIMALS was hardcoded to 6, so a 9-decimal mint made a correct
    // payment read as a 1000x mismatch — and, in the other direction, a
    // 3-decimal mint would have accepted a thousandth of the price. The
    // transaction reports the mint's decimals; use them.
    let mock = Mock::routed(
        &sig_list(&[plain_sig_entry("5xSig", 5)]),
        &[("5xSig", tx_with_decimals("1500000000", 9))], // 1.5 at 9 decimals
    );
    let mut config = cfg();
    config.token_decimals = 9;
    let v = verify_payment(&pay_args(), &config, mock, NOW).unwrap();
    assert!(
        matches!(v, Verdict::Paid { .. }),
        "1.5 of a 9-decimal mint must verify against a 1.5 price; got {v:?}"
    );
}

#[test]
fn underpay_in_a_non_six_decimal_mint_is_still_a_mismatch() {
    // The scaling fix must not become a way to underpay: 1.5 at 9 decimals is
    // 1_500_000_000, and 1_500_000 of that mint is 0.0015.
    let mock = Mock::routed(
        &sig_list(&[plain_sig_entry("5xSig", 5)]),
        &[("5xSig", tx_with_decimals("1500000", 9))],
    );
    let mut config = cfg();
    config.token_decimals = 9;
    let v = verify_payment(&pay_args(), &config, mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Mismatch { .. }), "got {v:?}");
}

// ── Fix B fixtures: the fulfillment marker ───────────────────────────────────

/// A signature-list entry for a fulfillment marker: a memo tx carrying the
/// PKFUL1 tag and naming this charge.
fn marker_sig_entry(signature: &str, age: u64) -> String {
    marker_sig_entry_for_payment(signature, age, PAYMENT_SIGNATURE)
}

fn marker_sig_entry_for_payment(signature: &str, age: u64, payment_signature: &str) -> String {
    let memo = fulfillment_memo_for(payment_signature);
    marker_sig_entry_with_memo(signature, age, &memo)
}

fn marker_sig_entry_with_memo(signature: &str, age: u64, memo: &str) -> String {
    let bt = NOW - age;
    serde_json::json!({
        "signature":signature,
        "slot":101,
        "err":null,
        "blockTime":bt,
        "memo":format!("[{}] {memo}", memo.len()),
        "confirmationStatus":"finalized"
    })
    .to_string()
}

fn sig_entry_without_memo(signature: &str, age: u64) -> String {
    serde_json::json!({
        "signature": signature,
        "slot": 101,
        "err": null,
        "blockTime": NOW - age,
        "memo": null,
        "confirmationStatus": "finalized"
    })
    .to_string()
}

fn fulfillment_memo_for(payment_signature: &str) -> String {
    serde_json::json!({
        "v":1,
        "dev":"kiosk01",
        "seq":3,
        "ts":NOW - 3,
        "tag":FULFILLMENT_TAG,
        "ref":REFERENCE,
        "item":"cold_drink",
        "payment_sig":payment_signature,
        "prev":null
    })
    .to_string()
}

/// A plain (non-marker) signature-list entry.
fn plain_sig_entry(signature: &str, age: u64) -> String {
    payment_sig_entry(signature, NOW - age, REFERENCE, "cold_drink")
}

fn sig_list(entries: &[String]) -> String {
    format!("[{}]", entries.join(","))
}

/// A getTransaction result for a marker: memo-only, signed by `signer`, with
/// the charge reference present as a read-only key.
fn marker_tx(signer: &str) -> String {
    marker_tx_for_payment(signer, PAYMENT_SIGNATURE)
}

fn marker_tx_for_payment(signer: &str, payment_signature: &str) -> String {
    let memo = fulfillment_memo_for(payment_signature);
    marker_tx_with_memo(signer, &memo)
}

fn marker_tx_with_memo(signer: &str, memo: &str) -> String {
    serde_json::json!({
        "slot":101,
        "blockTime":NOW - 3,
        "meta":{"err":null,"preTokenBalances":[],"postTokenBalances":[]},
        "transaction":{"message":{
            "accountKeys":[
                {"pubkey":signer,"signer":true,"writable":true},
                {"pubkey":DEVICE,"signer":false,"writable":true},
                {"pubkey":REFERENCE,"signer":false,"writable":false},
                {"pubkey":SYSTEM_PROGRAM,"signer":false,"writable":false},
                {"pubkey":MEMO_PROGRAM,"signer":false,"writable":false}
            ],
            "instructions":[
                {"program":"system","programId":SYSTEM_PROGRAM,"parsed":{
                    "type":"advanceNonce",
                    "info":{"nonceAccount":DEVICE,"nonceAuthority":signer}
                }},
                {"program":"spl-memo","programId":MEMO_PROGRAM,"parsed":memo}
            ]
        }}
    })
    .to_string()
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
            ("device_address", DEVICE),
            ("device_id", "kiosk01"),
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
            payment_block_time_s,
            reference,
            item_id,
            amount,
            recipient,
            mint,
            token_decimals,
            payment_window_s,
        } => {
            assert_eq!(payer, "PayerAcct");
            assert_eq!(signature, "5xSig");
            assert_eq!(slot, 100);
            assert_eq!(payment_block_time_s, NOW - 5);
            assert_eq!(reference, REFERENCE);
            assert_eq!(item_id, "cold_drink");
            assert_eq!(amount, "1.5");
            assert_eq!(recipient, MERCHANT);
            assert_eq!(mint, DEFAULT_USDC_MINT);
            assert_eq!(token_decimals, 6);
            assert_eq!(payment_window_s, 900);
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
fn transfer_with_multiple_references_is_rejected() {
    let mut transaction = payment_tx_value(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        "1500000",
        6,
        "null",
        true,
    );
    transaction["transaction"]["message"]["instructions"][2]["parsed"]["info"]["signers"] =
        serde_json::json!([REFERENCE, OTHER_MINT]);
    let mock = Mock::full(&one_sig(5), &transaction.to_string());
    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Mismatch { .. }),
        "got {verdict:?}"
    );
}

#[test]
fn equal_price_item_swap_is_rejected_by_instruction_memo() {
    let transaction = tx_for_item(
        "day_pass",
        MERCHANT,
        DEFAULT_USDC_MINT,
        "1500000",
        6,
        "null",
    );
    let mock = Mock::full(&one_sig_for(5, "cold_drink"), &transaction);
    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Mismatch { .. }),
        "got {verdict:?}"
    );
}

#[test]
fn internal_merchant_transfer_has_zero_net_credit_and_is_rejected() {
    let mut transaction = payment_tx_value(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        "1500000",
        6,
        "null",
        true,
    );
    transaction["meta"]["preTokenBalances"] = serde_json::json!([
        {"accountIndex":1,"mint":DEFAULT_USDC_MINT,"owner":MERCHANT,"uiTokenAmount":{"amount":"0","decimals":6}},
        {"accountIndex":2,"mint":DEFAULT_USDC_MINT,"owner":MERCHANT,"uiTokenAmount":{"amount":"1500000","decimals":6}}
    ]);
    transaction["meta"]["postTokenBalances"] = serde_json::json!([
        {"accountIndex":1,"mint":DEFAULT_USDC_MINT,"owner":MERCHANT,"uiTokenAmount":{"amount":"1500000","decimals":6}},
        {"accountIndex":2,"mint":DEFAULT_USDC_MINT,"owner":MERCHANT,"uiTokenAmount":{"amount":"0","decimals":6}}
    ]);
    let mock = Mock::full(&one_sig(5), &transaction.to_string());
    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Mismatch { .. }),
        "got {verdict:?}"
    );
}

#[test]
fn wrong_transfer_destination_is_rejected() {
    let mut transaction = payment_tx_value(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        "1500000",
        6,
        "null",
        true,
    );
    transaction["transaction"]["message"]["instructions"][2]["parsed"]["info"]["destination"] =
        serde_json::json!("SourceAta");
    let mock = Mock::full(&one_sig(5), &transaction.to_string());
    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Mismatch { .. }),
        "got {verdict:?}"
    );
}

#[test]
fn multiple_token_transfers_are_rejected() {
    let mut transaction = payment_tx_value(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        "1500000",
        6,
        "null",
        true,
    );
    let duplicate = transaction["transaction"]["message"]["instructions"][2].clone();
    transaction["transaction"]["message"]["instructions"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let mock = Mock::full(&one_sig(5), &transaction.to_string());
    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Mismatch { .. }),
        "got {verdict:?}"
    );
}

#[test]
fn missing_slot_is_a_decode_error() {
    let mut transaction = payment_tx_value(
        "cold_drink",
        MERCHANT,
        DEFAULT_USDC_MINT,
        "1500000",
        6,
        "null",
        true,
    );
    transaction.as_object_mut().unwrap().remove("slot");
    let mock = Mock::full(&one_sig(5), &transaction.to_string());
    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "got {result:?}"
    );
}

#[test]
fn far_future_payment_blocktime_is_never_paid() {
    let entry = payment_sig_entry("5xSig", NOW + 31, REFERENCE, "cold_drink");
    let mock = Mock::routed(
        &sig_list(&[entry]),
        &[("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"))],
    );
    let verdict = verify_payment(&pay_args(), &cfg(), &mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Mismatch { .. }),
        "got {verdict:?}"
    );
    assert_eq!(
        mock.tx_calls.get(),
        1,
        "candidate must be fetched once so an authenticated future schema cannot bypass marker handling"
    );
}

#[test]
fn old_payment_is_reported_for_trusted_quote_time_validation() {
    // Observation can happen after an outage. Watch reports the actual landed
    // time; the trusted claim checks it against the immutable quote expiry.
    let mut config = cfg();
    config.payment_window_s = 60;
    let mock = Mock::full(
        &one_sig(3600),
        &tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
    );
    let v = verify_payment(&pay_args(), &config, mock, NOW).unwrap();
    assert!(
        matches!(
            v,
            Verdict::Paid {
                payment_block_time_s,
                payment_window_s: 60,
                ..
            } if payment_block_time_s == NOW - 3600
        ),
        "got {v:?}"
    );
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
        &one_sig_for(5, "day_pass"),
        &tx_for_item(
            "day_pass",
            MERCHANT,
            DEFAULT_USDC_MINT,
            "5000000",
            6,
            "null",
        ),
    );
    let v = verify_payment(&args_for("day_pass"), &cfg(), mock, NOW).unwrap();
    assert!(matches!(v, Verdict::Paid { .. }), "got {v:?}");
}

#[test]
fn underpay_for_item_is_mismatch() {
    // config prices day_pass at 5; a 1 USDC payment must not clear it.
    let mock = Mock::full(
        &one_sig_for(5, "day_pass"),
        &tx_for_item(
            "day_pass",
            MERCHANT,
            DEFAULT_USDC_MINT,
            "1000000",
            6,
            "null",
        ),
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

// ── Bounded on-chain replay marker ───────────────────────────────────────────

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
            plain_sig_entry(PAYMENT_SIGNATURE, 5),
        ]),
        &[
            ("MarkerSig", marker_tx(AUTHORITY)),
            (
                PAYMENT_SIGNATURE,
                tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
            ),
        ],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert_eq!(v, Verdict::AlreadyFulfilled, "got {v:?}");
    assert!(!v.is_paid(), "AlreadyFulfilled must never gate the relay");
}

#[test]
fn fulfillment_marker_cannot_predate_its_payment() {
    let mut marker_entry: serde_json::Value =
        serde_json::from_str(&marker_sig_entry("MarkerSig", 2)).unwrap();
    marker_entry["slot"] = serde_json::json!(99);
    let mock = Mock::routed(
        &sig_list(&[
            marker_entry.to_string(),
            plain_sig_entry(PAYMENT_SIGNATURE, 5),
        ]),
        &[
            ("MarkerSig", marker_tx(AUTHORITY)),
            (
                PAYMENT_SIGNATURE,
                tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
            ),
        ],
    );
    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "a fulfillment marker cannot prove delivery before its payment lands: {result:?}"
    );
}

#[test]
fn fulfillment_marker_must_name_a_payment_that_verifies() {
    let unknown_payment = kiosk_core::b58::encode(&[8u8; 64]);
    let marker_entry = marker_sig_entry_for_payment("MarkerSig", 2, &unknown_payment);
    let marker_transaction = marker_tx_for_payment(AUTHORITY, &unknown_payment);
    let mock = Mock::routed(
        &sig_list(&[marker_entry]),
        &[("MarkerSig", marker_transaction)],
    );
    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "an authority marker cannot cite an arbitrary signature: {result:?}"
    );
}

#[test]
fn malformed_authority_signed_marker_fails_closed() {
    let mut malformed: serde_json::Value = serde_json::from_str(&marker_tx(AUTHORITY)).unwrap();
    malformed["transaction"]["message"]
        .as_object_mut()
        .unwrap()
        .remove("instructions");
    let mock = Mock::routed(
        &sig_list(&[
            marker_sig_entry("MarkerSig", 2),
            plain_sig_entry("5xSig", 5),
        ]),
        &[
            ("MarkerSig", malformed.to_string()),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );
    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "got {result:?}"
    );
}

#[test]
fn authority_signed_marker_without_listed_memo_fails_closed() {
    let mut no_memo_tx: serde_json::Value = serde_json::from_str(&marker_tx(AUTHORITY)).unwrap();
    no_memo_tx["transaction"]["message"]["instructions"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let mock = Mock::routed(
        &sig_list(&[
            sig_entry_without_memo("NoMemoSig", 2),
            plain_sig_entry("5xSig", 5),
        ]),
        &[
            ("NoMemoSig", no_memo_tx.to_string()),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );

    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(ref message)) if message.contains("no memo")),
        "an authority-signed write cannot hide from marker validation by omitting its memo: {result:?}"
    );
}

#[test]
fn outsider_transaction_without_memo_cannot_dos_payment() {
    let mock = Mock::routed(
        &sig_list(&[
            sig_entry_without_memo("OutsiderNoMemo", 2),
            plain_sig_entry("5xSig", 5),
        ]),
        &[
            ("OutsiderNoMemo", junk_tx()),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );

    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        matches!(verdict, Verdict::Paid { .. }),
        "an outsider's memo-less transaction must not block a real payment: {verdict:?}"
    );
}

#[test]
fn authority_signed_future_marker_schema_fails_closed() {
    let mut memo: serde_json::Value =
        serde_json::from_str(&fulfillment_memo_for(PAYMENT_SIGNATURE)).unwrap();
    memo["future_field"] = serde_json::json!(true);
    let memo = memo.to_string();
    let mock = Mock::routed(
        &sig_list(&[
            marker_sig_entry_with_memo("MarkerSig", 2, &memo),
            plain_sig_entry(PAYMENT_SIGNATURE, 5),
        ]),
        &[
            ("MarkerSig", marker_tx_with_memo(AUTHORITY, &memo)),
            (
                PAYMENT_SIGNATURE,
                tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
            ),
        ],
    );
    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "an authenticated unknown marker schema must not fall through to Paid: {result:?}"
    );
}

#[test]
fn authority_signed_future_marker_version_cannot_reopen_payment() {
    let mut memo: serde_json::Value =
        serde_json::from_str(&fulfillment_memo_for(PAYMENT_SIGNATURE)).unwrap();
    memo["tag"] = serde_json::json!("PKFUL2");
    memo["v"] = serde_json::json!(2);
    let memo = memo.to_string();
    let mock = Mock::routed(
        &sig_list(&[
            marker_sig_entry_with_memo("FutureMarker", 2, &memo),
            plain_sig_entry(PAYMENT_SIGNATURE, 5),
        ]),
        &[
            ("FutureMarker", marker_tx_with_memo(AUTHORITY, &memo)),
            (
                PAYMENT_SIGNATURE,
                tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
            ),
        ],
    );
    let result = verify_payment(&pay_args(), &cfg(), mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "an authenticated future marker version must not fall through to Paid: {result:?}"
    );
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
fn outsider_marker_with_wrong_device_and_item_cannot_dos_payment() {
    let spoofed_memo = serde_json::json!({
        "v":1,
        "dev":"attacker-device",
        "seq":3,
        "ts":NOW - 2,
        "tag":FULFILLMENT_TAG,
        "ref":REFERENCE,
        "item":"day_pass",
        "payment_sig":PAYMENT_SIGNATURE,
        "prev":null
    })
    .to_string();
    let mock = Mock::routed(
        &sig_list(&[
            marker_sig_entry_with_memo("SpoofSig", 2, &spoofed_memo),
            plain_sig_entry("5xSig", 5),
        ]),
        &[
            ("SpoofSig", marker_tx_with_memo(ATTACKER, &spoofed_memo)),
            ("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null")),
        ],
    );
    let verdict = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(matches!(verdict, Verdict::Paid { .. }), "got {verdict:?}");
}

#[test]
fn forged_marker_crowd_cannot_hide_an_authentic_marker() {
    // Regression: a secondary marker-authentication budget of three let an
    // attacker put three newer forged markers ahead of the genuine one. The
    // genuine marker remained visible in the ten-entry reference window but
    // was never checked, so the old payment became PAID again.
    let mock = Mock::routed(
        &sig_list(&[
            marker_sig_entry("Spoof1", 1),
            marker_sig_entry("Spoof2", 2),
            marker_sig_entry("Spoof3", 3),
            marker_sig_entry("MarkerSig", 4),
            plain_sig_entry(PAYMENT_SIGNATURE, 5),
        ]),
        &[
            ("Spoof1", marker_tx(ATTACKER)),
            ("Spoof2", marker_tx(ATTACKER)),
            ("Spoof3", marker_tx(ATTACKER)),
            ("MarkerSig", marker_tx(AUTHORITY)),
            (
                PAYMENT_SIGNATURE,
                tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
            ),
        ],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert_eq!(v, Verdict::AlreadyFulfilled, "got {v:?}");
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
            plain_sig_entry(PAYMENT_SIGNATURE, 5),
        ]),
        &[
            ("MarkerSig", marker_tx(AUTHORITY)),
            (
                PAYMENT_SIGNATURE,
                tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"),
            ),
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

// ── #21 / #22: the actuating verdict must not fail open ─────────────────────

/// A signature entry the node returned without a `blockTime`.
fn sig_entry_no_blocktime(signature: &str) -> String {
    let mut entry: serde_json::Value =
        serde_json::from_str(&payment_sig_entry(signature, NOW, REFERENCE, "cold_drink")).unwrap();
    entry.as_object_mut().unwrap().remove("blockTime");
    entry.to_string()
}

fn cfg_with_finality(finality: &str) -> Result<WatchConfig, WatchError> {
    WatchConfig::from_section(
        &[
            ("rpc_url", RPC),
            ("merchant_address", MERCHANT),
            ("price_list", PRICE_LIST),
            ("device_authority", AUTHORITY),
            ("device_address", DEVICE),
            ("device_id", "kiosk01"),
            ("finality", finality),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    )
}

#[test]
fn invalid_operator_time_policy_fails_at_config_load() {
    for (key, value) in [
        ("payment_window_s", "0"),
        ("payment_window_s", "86401"),
        ("heartbeat_max_silence_s", "not-a-number"),
    ] {
        let section = [
            ("rpc_url", RPC),
            ("merchant_address", MERCHANT),
            ("price_list", PRICE_LIST),
            (key, value),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert!(
            WatchConfig::from_section(&section).is_err(),
            "{key}={value}"
        );
    }
}

#[test]
fn invalid_token_decimal_policy_fails_at_config_load() {
    for (decimals, price) in [
        ("19", "item:1"),
        ("wat", "item:1"),
        ("2", "item:1.001"),
        ("6", "item:.5"),
        ("6", "item:1."),
        ("18", "item:100"),
    ] {
        let section = [
            ("rpc_url", RPC),
            ("merchant_address", MERCHANT),
            ("token_decimals", decimals),
            ("price_list", price),
        ]
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
        assert!(
            WatchConfig::from_section(&section).is_err(),
            "{decimals} {price}"
        );
    }
}

#[test]
fn candidate_without_blocktime_is_never_paid() {
    // No blockTime means the trusted claim cannot compare payment landing time
    // with the immutable quote lifetime. Unverifiable must mean not-Paid.
    let mock = Mock::routed(
        &sig_list(&[sig_entry_no_blocktime("5xSig")]),
        &[("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"))],
    );
    let v = verify_payment(&pay_args(), &cfg(), mock, NOW).unwrap();
    assert!(
        !matches!(v, Verdict::Paid { .. }),
        "a payment with no claimable landing time must never be Paid; got {v:?}"
    );
}

#[test]
fn weaker_commitments_are_refused_for_an_actuating_verdict() {
    // `processed` can be rolled back and `confirmed` is only reorg-unlikely.
    // Dispensing a physical item is not reversible, so the verdict that opens
    // a relay requires economic irreversibility.
    for weak in ["processed", "confirmed"] {
        let cfg = cfg_with_finality(weak).unwrap();
        let mock = Mock::sigs("[]");
        let r = verify_payment(&pay_args(), &cfg, &mock, NOW);
        match r {
            Err(WatchError::Config(m)) => assert!(
                m.contains("finalized"),
                "error should name the required commitment: {m}"
            ),
            other => panic!("`{weak}` must be refused for actuation, got {other:?}"),
        }
        assert_eq!(mock.sig_calls.get(), 0, "must fail before touching the RPC");
    }
}

#[test]
fn finalized_commitment_verifies_normally() {
    let mock = Mock::routed(
        &sig_list(&[plain_sig_entry("5xSig", 5)]),
        &[("5xSig", tx(MERCHANT, DEFAULT_USDC_MINT, "1500000", "null"))],
    );
    let v = verify_payment(
        &pay_args(),
        &cfg_with_finality("finalized").unwrap(),
        mock,
        NOW,
    )
    .unwrap();
    assert!(matches!(v, Verdict::Paid { .. }), "got {v:?}");
}

#[test]
fn finality_defaults_to_finalized() {
    // The default has to be the safe one: an operator who never thinks about
    // commitment gets irreversibility, not a silent downgrade.
    assert_eq!(cfg().finality, "finalized");
}

#[test]
fn heartbeat_still_accepts_a_weaker_commitment() {
    // The finalized requirement is about actuation. Liveness monitoring is not
    // actuation, and forcing ~13s of extra latency on a heartbeat would only
    // make outage detection slower.
    let cfg = cfg_with_finality("confirmed").unwrap();
    let mock = Mock::routed(
        &heartbeat_sig_entry("S", 10, "kiosk01"),
        &[("S", heartbeat_tx(AUTHORITY, 10))],
    );
    let h = verify_heartbeat(&cfg, mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Live { .. }), "got {h:?}");
}

#[test]
fn zero_priced_item_is_rejected_at_config_load() {
    // A zero price makes `delta == expected_units` true for a transaction that
    // moved no money at all, so a free item would open the relay on any tx that
    // referenced the charge. kiosk-charge already refuses a non-positive
    // amount; the verifier must refuse it too, or the two disagree about what
    // is sellable and the weaker one wins.
    let section = [
        ("rpc_url", RPC),
        ("merchant_address", MERCHANT),
        ("price_list", "freebie:0"),
        ("device_authority", AUTHORITY),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let err = WatchConfig::from_section(&section).unwrap_err();
    match err {
        WatchError::Config(m) => assert!(
            m.contains("freebie") && (m.contains("positive") || m.contains("greater")),
            "error should name the item and say the price must be positive: {m}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }
}

#[test]
fn missing_device_authority_fails_closed() {
    // Without it no marker can be authenticated, so the on-chain replay
    // barrier cannot be enforced. Refuse rather than disable the check.
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
fn payment_machine_output_is_json_and_routes_only_paid() {
    let paid = Verdict::Paid {
        payer: ATTACKER.into(),
        signature: "5xSig".into(),
        slot: 42,
        payment_block_time_s: NOW - 5,
        reference: REFERENCE.into(),
        item_id: "cold_drink".into(),
        amount: "1.5".into(),
        recipient: MERCHANT.into(),
        mint: DEFAULT_USDC_MINT.into(),
        token_decimals: 6,
        payment_window_s: 900,
    };
    let paid_json: serde_json::Value = serde_json::from_str(&paid.machine_output()).unwrap();
    assert_eq!(paid_json["success"], true);
    assert_eq!(paid_json["status"], "paid");
    assert_eq!(paid_json["amount"], "1.5");
    assert_eq!(paid_json["recipient"], MERCHANT);
    assert_eq!(paid_json["mint"], DEFAULT_USDC_MINT);
    assert_eq!(paid_json["token_decimals"], 6);
    assert_eq!(paid_json["payment_block_time_s"], NOW - 5);
    assert_eq!(paid_json["payment_window_s"], 900);

    for verdict in [Verdict::Pending, Verdict::AlreadyFulfilled] {
        let value: serde_json::Value = serde_json::from_str(&verdict.machine_output()).unwrap();
        assert_eq!(value["success"], false, "got {value}");
    }
}

#[test]
fn heartbeat_machine_output_is_json_and_routes_only_live() {
    let live = Heartbeat::Live {
        signature: "5xSig".into(),
        age_s: 3,
    };
    let stale = Heartbeat::Stale {
        signature: "5xSig".into(),
        age_s: 300,
    };
    let live_json: serde_json::Value = serde_json::from_str(&live.machine_output()).unwrap();
    let stale_json: serde_json::Value = serde_json::from_str(&stale.machine_output()).unwrap();
    assert_eq!(live_json["success"], true);
    assert_eq!(live_json["status"], "live");
    assert_eq!(stale_json["success"], false);
    assert_eq!(stale_json["status"], "stale");
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

fn heartbeat_sig_entry(signature: &str, age_s: u64, device_id: &str) -> String {
    let memo = heartbeat_memo(device_id, NOW - age_s);
    heartbeat_sig_entry_with_times(signature, NOW - age_s, &memo)
}

fn heartbeat_sig_entry_with_times(signature: &str, block_time: u64, memo: &str) -> String {
    serde_json::json!([{
        "signature":signature,
        "slot":100,
        "err":null,
        "blockTime":block_time,
        "memo":format!("[{}] {memo}", memo.len())
    }])
    .to_string()
}

fn heartbeat_memo(device_id: &str, ts: u64) -> String {
    serde_json::json!({
        "v":1,
        "dev":device_id,
        "seq":7,
        "ts":ts,
        "event":"heartbeat",
        "prev":null
    })
    .to_string()
}

fn heartbeat_tx(signer: &str, age_s: u64) -> String {
    heartbeat_tx_at(signer, NOW - age_s)
}

fn heartbeat_tx_at(signer: &str, ts: u64) -> String {
    let memo = heartbeat_memo("kiosk01", ts);
    heartbeat_tx_with_memo(signer, &memo)
}

fn heartbeat_tx_with_memo(signer: &str, memo: &str) -> String {
    serde_json::json!({
        "meta":{"err":null},
        "transaction":{"message":{
            "accountKeys":[
                {"pubkey":signer,"signer":true,"writable":true},
                {"pubkey":DEVICE,"signer":false,"writable":true},
                {"pubkey":SYSTEM_PROGRAM,"signer":false,"writable":false},
                {"pubkey":MEMO_PROGRAM,"signer":false,"writable":false}
            ],
            "instructions":[
                {"program":"system","programId":SYSTEM_PROGRAM,"parsed":{
                    "type":"advanceNonce",
                    "info":{"nonceAccount":DEVICE,"nonceAuthority":signer}
                }},
                {"program":"spl-memo","programId":MEMO_PROGRAM,"parsed":memo}
            ]
        }}
    })
    .to_string()
}

fn nonce_initialization_tx() -> String {
    serde_json::json!({
        "meta":{"err":null},
        "transaction":{"message":{
            "accountKeys":[
                {"pubkey":AUTHORITY,"signer":true,"writable":true},
                {"pubkey":DEVICE,"signer":true,"writable":true},
                {"pubkey":SYSTEM_PROGRAM,"signer":false,"writable":false}
            ],
            "instructions":[
                {"program":"system","programId":SYSTEM_PROGRAM,"parsed":{
                    "type":"createAccount",
                    "info":{
                        "source":AUTHORITY,
                        "newAccount":DEVICE,
                        "lamports":1_500_000,
                        "space":80,
                        "owner":SYSTEM_PROGRAM
                    }
                }},
                {"program":"system","programId":SYSTEM_PROGRAM,"parsed":{
                    "type":"initializeNonce",
                    "info":{
                        "nonceAccount":DEVICE,
                        "recentBlockhashesSysvar":RECENT_BLOCKHASHES_SYSVAR,
                        "rentSysvar":RENT_SYSVAR,
                        "nonceAuthority":AUTHORITY
                    }
                }}
            ]
        }}
    })
    .to_string()
}

#[test]
fn far_future_heartbeat_is_never_live() {
    let timestamp = NOW + 31;
    let memo = heartbeat_memo("kiosk01", timestamp);
    let entries = serde_json::json!([{
        "signature":"FutureHeartbeat",
        "slot":100,
        "err":null,
        "blockTime":timestamp,
        "memo":format!("[{}] {memo}", memo.len())
    }])
    .to_string();
    let mock = Mock::routed(
        &entries,
        &[("FutureHeartbeat", heartbeat_tx_at(AUTHORITY, timestamp))],
    );
    let heartbeat = verify_heartbeat(&cfg(), &mock, NOW);
    assert!(matches!(heartbeat, Err(WatchError::Decode(_))));
    assert_eq!(
        mock.tx_calls.get(),
        1,
        "candidate must be authenticated before its timestamp can fail closed"
    );
}

#[test]
fn delayed_durable_nonce_heartbeat_uses_signed_observation_time() {
    let memo = heartbeat_memo("kiosk01", NOW - 3600);
    let entries = heartbeat_sig_entry_with_times("DelayedHeartbeat", NOW - 2, &memo);
    let mock = Mock::routed(
        &entries,
        &[("DelayedHeartbeat", heartbeat_tx_with_memo(AUTHORITY, &memo))],
    );

    let heartbeat = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert_eq!(
        heartbeat,
        Heartbeat::Stale {
            signature: "DelayedHeartbeat".into(),
            age_s: 3600,
        },
        "a recently landed old durable-nonce artifact must remain stale"
    );
}

#[test]
fn heartbeat_signed_time_cannot_postdate_now_or_landing_beyond_skew() {
    for (signature, signed_ts, block_time) in [
        ("FutureSignedTime", NOW + 31, NOW),
        ("AfterLanding", NOW, NOW - 31),
    ] {
        let memo = heartbeat_memo("kiosk01", signed_ts);
        let entries = heartbeat_sig_entry_with_times(signature, block_time, &memo);
        let mock = Mock::routed(
            &entries,
            &[(signature, heartbeat_tx_with_memo(AUTHORITY, &memo))],
        );
        let heartbeat = verify_heartbeat(&cfg(), mock, NOW);
        assert!(
            matches!(heartbeat, Err(WatchError::Decode(_))),
            "invalid signed/landing time relationship was accepted for {signature}: {heartbeat:?}"
        );
    }
}

#[test]
fn authority_signed_heartbeat_without_listed_memo_fails_closed() {
    let mut no_memo_tx: serde_json::Value =
        serde_json::from_str(&heartbeat_tx(AUTHORITY, 2)).unwrap();
    no_memo_tx["transaction"]["message"]["instructions"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let older = heartbeat_sig_entry("OlderValid", 30, "kiosk01")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let mock = Mock::routed(
        &sig_list(&[sig_entry_without_memo("NoMemo", 2), older]),
        &[
            ("NoMemo", no_memo_tx.to_string()),
            ("OlderValid", heartbeat_tx(AUTHORITY, 30)),
        ],
    );

    let heartbeat = verify_heartbeat(&cfg(), mock, NOW);
    assert!(
        matches!(heartbeat, Err(WatchError::Decode(ref message)) if message.contains("memo-less") || message.contains("no memo")),
        "an authority-signed memo-less device write cannot reopen an older Live verdict: {heartbeat:?}"
    );
}

#[test]
fn fresh_nonce_initialization_is_not_a_malformed_heartbeat() {
    let mock = Mock::routed(
        &sig_list(&[sig_entry_without_memo("NonceInit", 2)]),
        &[("NonceInit", nonce_initialization_tx())],
    );

    let heartbeat = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert_eq!(heartbeat, Heartbeat::Missing);
}

#[test]
fn nonce_initialization_hides_heartbeat_from_previous_incarnation() {
    let older = heartbeat_sig_entry("OlderValid", 30, "kiosk01")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let mock = Mock::routed(
        &sig_list(&[sig_entry_without_memo("NonceInit", 2), older]),
        &[
            ("NonceInit", nonce_initialization_tx()),
            ("OlderValid", heartbeat_tx(AUTHORITY, 30)),
        ],
    );

    let heartbeat = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert_eq!(heartbeat, Heartbeat::Missing);
}

#[test]
fn outsider_heartbeat_transaction_without_memo_is_ignored() {
    let older = heartbeat_sig_entry("OlderValid", 30, "kiosk01")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let mock = Mock::routed(
        &sig_list(&[sig_entry_without_memo("OutsiderNoMemo", 2), older]),
        &[
            ("OutsiderNoMemo", junk_tx()),
            ("OlderValid", heartbeat_tx(AUTHORITY, 30)),
        ],
    );

    let heartbeat = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert_eq!(
        heartbeat,
        Heartbeat::Live {
            signature: "OlderValid".into(),
            age_s: 30,
        }
    );
}

#[test]
fn heartbeat_within_clock_skew_is_live_with_zero_age() {
    for skew_s in [1, 30] {
        let timestamp = NOW + skew_s;
        let memo = heartbeat_memo("kiosk01", timestamp);
        let entries = serde_json::json!([{
            "signature":format!("FutureHeartbeat{skew_s}"),
            "slot":100,
            "err":null,
            "blockTime":timestamp,
            "memo":format!("[{}] {memo}", memo.len())
        }])
        .to_string();
        let signature = format!("FutureHeartbeat{skew_s}");
        let mock = Mock::routed(
            &entries,
            &[(signature.as_str(), heartbeat_tx_at(AUTHORITY, timestamp))],
        );
        let heartbeat = verify_heartbeat(&cfg(), &mock, NOW).unwrap();
        assert_eq!(
            heartbeat,
            Heartbeat::Live {
                signature,
                age_s: 0,
            }
        );
    }
}

#[test]
fn newer_authority_attestation_for_wrong_device_fails_closed() {
    let wrong_memo = heartbeat_memo("another-kiosk", NOW - 2);
    let wrong_entry = serde_json::json!({
        "signature":"WrongDevice",
        "slot":101,
        "err":null,
        "blockTime":NOW - 2,
        "memo":format!("[{}] {wrong_memo}", wrong_memo.len())
    })
    .to_string();
    let mock = Mock::routed(
        &sig_list(&[
            wrong_entry,
            heartbeat_sig_entry("OlderValid", 30, "kiosk01")
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string(),
        ]),
        &[
            (
                "WrongDevice",
                heartbeat_tx_with_memo(AUTHORITY, &wrong_memo),
            ),
            ("OlderValid", heartbeat_tx(AUTHORITY, 30)),
        ],
    );
    let result = verify_heartbeat(&cfg(), &mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "a newer authenticated wrong-device write must not fall through to an older Live verdict: {result:?}"
    );
}

#[test]
fn newer_authority_attestation_version_fails_closed() {
    let mut future: serde_json::Value =
        serde_json::from_str(&heartbeat_memo("kiosk01", NOW - 2)).unwrap();
    future["v"] = serde_json::json!(2);
    let future_memo = future.to_string();
    let future_entry = serde_json::json!({
        "signature":"FutureVersion",
        "slot":101,
        "err":null,
        "blockTime":NOW - 2,
        "memo":format!("[{}] {future_memo}", future_memo.len())
    })
    .to_string();
    let mock = Mock::routed(
        &sig_list(&[
            future_entry,
            heartbeat_sig_entry("OlderValid", 30, "kiosk01")
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string(),
        ]),
        &[
            (
                "FutureVersion",
                heartbeat_tx_with_memo(AUTHORITY, &future_memo),
            ),
            ("OlderValid", heartbeat_tx(AUTHORITY, 30)),
        ],
    );
    let result = verify_heartbeat(&cfg(), &mock, NOW);
    assert!(
        matches!(result, Err(WatchError::Decode(_))),
        "an authenticated future attestation version must not fall through to an older Live verdict: {result:?}"
    );
}

#[test]
fn heartbeat_live_when_recent() {
    let mock = Mock::routed(
        &heartbeat_sig_entry("Heartbeat", 30, "kiosk01"),
        &[("Heartbeat", heartbeat_tx(AUTHORITY, 30))],
    );
    let h = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Live { .. }), "got {h:?}");
}

#[test]
fn heartbeat_stale_when_old() {
    let mock = Mock::routed(
        &heartbeat_sig_entry("Heartbeat", 3600, "kiosk01"),
        &[("Heartbeat", heartbeat_tx(AUTHORITY, 3600))],
    );
    let h = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Stale { .. }), "got {h:?}");
}

#[test]
fn heartbeat_missing_when_no_signatures() {
    let mock = Mock::sigs("[]");
    let h = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert!(matches!(h, Heartbeat::Missing), "got {h:?}");
}

#[test]
fn heartbeat_rpc_error_is_err_never_live() {
    let mock = Mock::new(Err(RpcError::Transport("boom".into())), Ok(wrap("null")));
    let r = verify_heartbeat(&cfg(), mock, NOW);
    assert!(r.is_err());
    assert!(!matches!(r, Ok(Heartbeat::Live { .. })));
}

#[test]
fn heartbeat_missing_operator_device_address_fails_closed() {
    let config = WatchConfig::from_section(
        &[
            ("rpc_url", RPC),
            ("merchant_address", MERCHANT),
            ("price_list", PRICE_LIST),
            ("device_authority", AUTHORITY),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    )
    .unwrap();
    let mock = Mock::sigs("[]");
    let r = verify_heartbeat(&config, mock, NOW);
    assert!(matches!(r, Err(WatchError::Config(_))), "got {r:?}");
}

#[test]
fn spoofed_heartbeat_is_ignored() {
    let mock = Mock::routed(
        &heartbeat_sig_entry("Spoof", 10, "kiosk01"),
        &[("Spoof", heartbeat_tx(ATTACKER, 10))],
    );
    let h = verify_heartbeat(&cfg(), mock, NOW).unwrap();
    assert_eq!(h, Heartbeat::Missing);
}
