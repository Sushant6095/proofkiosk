//! Host tests for the kiosk-attest core. RPC (chain recovery + nonce read) is
//! mocked; NO live network. Injection drills come first. The load-bearing test
//! is structural: the built transaction can contain ONLY the Memo and System
//! (advance-nonce) programs — a transfer is not expressible.

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use kiosk_attest::attest::{
    execute_attest, unix_timestamp, AttestArgs, AttestConfig, AttestError, AttestOutput,
};
use kiosk_core::rpc::{RpcError, RpcTransport};
use kiosk_core::{b58, b64, memo, nonce};

const NONCE_AUTHORITY: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
const NONCE_ACCOUNT: &str = "So11111111111111111111111111111111111111112";
const RPC: &str = "https://api.devnet.solana.com";
const NOW: u64 = 1_700_000_000;
const RENT_SYSVAR: &str = "SysvarRent111111111111111111111111111111111";

// ── mock: dispatch by method; chain sigs + nonce account info ────────────────

struct Mock {
    sigs: Result<String, RpcError>,
    account: Result<String, RpcError>,
    transactions: HashMap<String, String>,
    sig_calls: std::cell::Cell<u32>,
    account_calls: std::cell::Cell<u32>,
    tx_calls: std::cell::Cell<u32>,
    call_order: std::cell::RefCell<Vec<&'static str>>,
    signature_min_slots: std::cell::RefCell<Vec<Option<u64>>>,
}
impl Mock {
    fn build(sigs: Result<String, RpcError>, account: Result<String, RpcError>) -> Self {
        Self {
            sigs,
            account,
            transactions: HashMap::new(),
            sig_calls: std::cell::Cell::new(0),
            account_calls: std::cell::Cell::new(0),
            tx_calls: std::cell::Cell::new(0),
            call_order: std::cell::RefCell::new(Vec::new()),
            signature_min_slots: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn with_transaction(mut self, signature: &str, response: String) -> Self {
        self.transactions.insert(signature.to_string(), response);
        self
    }
}
impl RpcTransport for Mock {
    fn send(&self, req: &str) -> Result<String, RpcError> {
        if req.contains("getSignaturesForAddress") {
            self.call_order.borrow_mut().push("signatures");
            let request: serde_json::Value = serde_json::from_str(req)
                .map_err(|error| RpcError::Decode(format!("bad test request: {error}")))?;
            self.signature_min_slots.borrow_mut().push(
                request
                    .pointer("/params/1/minContextSlot")
                    .and_then(serde_json::Value::as_u64),
            );
            self.sig_calls.set(self.sig_calls.get() + 1);
            self.sigs.clone()
        } else if req.contains("getAccountInfo") {
            self.call_order.borrow_mut().push("account");
            self.account_calls.set(self.account_calls.get() + 1);
            self.account.clone()
        } else if req.contains("getTransaction") {
            self.call_order.borrow_mut().push("transaction");
            self.tx_calls.set(self.tx_calls.get() + 1);
            let request: serde_json::Value = serde_json::from_str(req)
                .map_err(|e| RpcError::Decode(format!("bad test request: {e}")))?;
            let signature = request
                .get("params")
                .and_then(|p| p.get(0))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RpcError::Decode("missing transaction signature".into()))?;
            self.transactions
                .get(signature)
                .cloned()
                .ok_or_else(|| RpcError::Transport(format!("no test transaction for {signature}")))
        } else {
            Err(RpcError::Transport("unexpected method".into()))
        }
    }
}
fn env(result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result}}}"#)
}

fn env_value(result: serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": result }).to_string()
}

fn account_key(pubkey: &str, signer: bool, writable: bool) -> serde_json::Value {
    serde_json::json!({ "pubkey": pubkey, "signer": signer, "writable": writable })
}

fn system_instruction(kind: &str, info: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "program": "system",
        "programId": nonce::SYSTEM_PROGRAM_ID_B58,
        "parsed": { "type": kind, "info": info }
    })
}

fn transaction_response(nonce_is_signer: bool, instructions: Vec<serde_json::Value>) -> String {
    env_value(serde_json::json!({
        "meta": { "err": null },
        "transaction": {
            "message": {
                "accountKeys": [
                    account_key(NONCE_AUTHORITY, true, true),
                    account_key(NONCE_ACCOUNT, nonce_is_signer, true)
                ],
                "instructions": instructions
            }
        }
    }))
}

fn initialization_transaction() -> String {
    transaction_response(
        true,
        vec![
            system_instruction(
                "createAccount",
                serde_json::json!({
                    "source": NONCE_AUTHORITY,
                    "newAccount": NONCE_ACCOUNT,
                    "lamports": 1_500_000,
                    "space": 80,
                    "owner": nonce::SYSTEM_PROGRAM_ID_B58
                }),
            ),
            system_instruction(
                "initializeNonce",
                serde_json::json!({
                    "nonceAccount": NONCE_ACCOUNT,
                    "nonceAuthority": NONCE_AUTHORITY,
                    "recentBlockhashesSysvar": nonce::RECENT_BLOCKHASHES_SYSVAR_B58,
                    "rentSysvar": RENT_SYSVAR
                }),
            ),
        ],
    )
}

fn attestation_transaction(memo_text: &str) -> String {
    transaction_response(
        false,
        vec![
            system_instruction(
                "advanceNonce",
                serde_json::json!({
                    "nonceAccount": NONCE_ACCOUNT,
                    "nonceAuthority": NONCE_AUTHORITY,
                    "recentBlockhashesSysvar": nonce::RECENT_BLOCKHASHES_SYSVAR_B58
                }),
            ),
            serde_json::json!({
                "program": "spl-memo",
                "programId": memo::MEMO_PROGRAM_ID_B58,
                "parsed": memo_text
            }),
        ],
    )
}
/// A valid Current+Initialized nonce account owned by NONCE_AUTHORITY.
fn account_info() -> String {
    let authority = b58::decode_pubkey(NONCE_AUTHORITY).unwrap();
    let mut blob = Vec::new();
    blob.extend_from_slice(&1u32.to_le_bytes()); // version Current
    blob.extend_from_slice(&1u32.to_le_bytes()); // state Initialized
    blob.extend_from_slice(&authority);
    blob.extend_from_slice(&[0x11u8; 32]); // durable nonce (blockhash)
    blob.extend_from_slice(&5000u64.to_le_bytes());
    let data = b64::encode(&blob);
    env(&format!(
        r#"{{"context":{{"slot":1}},"value":{{"data":["{data}","base64"],"executable":false,"owner":"11111111111111111111111111111111"}}}}"#
    ))
}
fn genesis_chain(account: String) -> Mock {
    // A real fresh nonce already has its initialization transaction. Recovery
    // must recognize that as genesis instead of rejecting the first attestation.
    Mock::build(
        Ok(env_value(serde_json::json!([{
            "signature": "NonceInit",
            "slot": 1,
            "memo": null,
            "err": null
        }]))),
        Ok(account),
    )
    .with_transaction("NonceInit", initialization_transaction())
}

fn fresh_chain() -> Mock {
    genesis_chain(account_info())
}

fn chain_at_seq(seq: u64, sig: &str) -> Mock {
    assert!(seq > 0, "chain_at_seq fixture requires a predecessor");
    let signature_for = |current: u64| {
        if current == seq {
            sig.to_string()
        } else {
            format!("Chain{current}Of{sig}")
        }
    };
    let mut entries = Vec::new();
    let mut transactions = Vec::new();
    for current in (0..=seq).rev() {
        let signature = signature_for(current);
        let previous = (current > 0).then(|| signature_for(current - 1));
        let memo = serde_json::json!({
            "v": 1,
            "dev": "kiosk01",
            "seq": current,
            "ts": NOW - (seq - current),
            "metric": "temp_c",
            "val": 4.2,
            "prev": previous,
        })
        .to_string();
        entries.push(serde_json::json!({
            "signature": signature,
            "slot": current + 2,
            "err": null,
            "memo": format!("[{}] {memo}", memo.len())
        }));
        transactions.push((signature, attestation_transaction(&memo)));
    }
    entries.push(serde_json::json!({
        "signature": "NonceInit",
        "slot": 1,
        "err": null,
        "memo": null
    }));
    let mut mock = Mock::build(
        Ok(env_value(serde_json::Value::Array(entries))),
        Ok(account_info()),
    )
    .with_transaction("NonceInit", initialization_transaction());
    for (signature, transaction) in transactions {
        mock = mock.with_transaction(&signature, transaction);
    }
    mock
}

fn cfg() -> AttestConfig {
    let section: HashMap<String, String> = [
        ("rpc_url", RPC),
        ("device_id", "kiosk01"),
        ("nonce_account", NONCE_ACCOUNT),
        ("nonce_authority", NONCE_AUTHORITY),
        ("allowed_metrics", "temp_c:-40:85, humidity:0:100"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    AttestConfig::from_section(&section).unwrap()
}

fn config_with_allowed_metrics(allowed_metrics: &str) -> Result<AttestConfig, AttestError> {
    let section: HashMap<String, String> = [
        ("rpc_url", RPC),
        ("device_id", "kiosk01"),
        ("nonce_account", NONCE_ACCOUNT),
        ("nonce_authority", NONCE_AUTHORITY),
        ("allowed_metrics", allowed_metrics),
    ]
    .iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    AttestConfig::from_section(&section)
}

fn reading(metric: &str, value: f64) -> AttestArgs {
    AttestArgs {
        kind: Some("reading".into()),
        metric: Some(metric.into()),
        value: Some(value),
        ..Default::default()
    }
}

/// A charge reference pubkey — the same value kiosk-charge minted and
/// kiosk-watch scans.
const REFERENCE: &str = "Vote111111111111111111111111111111111111111";

fn fulfillment(reference: &str) -> AttestArgs {
    AttestArgs {
        kind: Some("fulfillment".into()),
        reference: Some(reference.into()),
        payment_sig: Some(kiosk_core::b58::encode(&[7u8; 64])),
        item: Some("cold_drink".into()),
        ..Default::default()
    }
}

/// The memo text of the built transaction (instruction 1 is always the memo).
fn memo_text(out: &AttestOutput) -> String {
    let ix = &out.message.instructions[1];
    String::from_utf8(ix.data.clone()).expect("memo is utf-8")
}

#[test]
fn the_allowlist_key_is_allowed_metrics_and_a_misspelling_fails_closed() {
    // The sensor SOP used to document this key as `metric_allowlist`, which the
    // code never reads. An operator following the docs got an EMPTY allowlist:
    // fail-closed (every reading refused), but it reads like a broken plugin
    // rather than a typo. Pin both halves — the real key works, and the wrong
    // one refuses rather than silently permitting anything.
    let with_wrong_key: HashMap<String, String> = [
        ("rpc_url", RPC),
        ("device_id", "kiosk01"),
        ("nonce_account", NONCE_ACCOUNT),
        ("nonce_authority", NONCE_AUTHORITY),
        ("metric_allowlist", "temp_c:-40:85"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let cfg_wrong = AttestConfig::from_section(&with_wrong_key).unwrap();
    assert!(
        cfg_wrong.allowed_metrics.is_empty(),
        "an unrecognised key must not populate the allowlist"
    );
    let r = execute_attest(&reading("temp_c", 4.2), &cfg_wrong, fresh_chain(), NOW);
    assert!(
        matches!(r, Err(AttestError::Rejected(_))),
        "a misspelled allowlist key must refuse every reading, got {r:?}"
    );

    // The documented key does populate it.
    assert_eq!(cfg().allowed_metrics.len(), 2);
}

#[test]
fn allowed_metrics_rejects_oversized_or_too_many_entries() {
    let oversized = "x".repeat(4097);
    assert!(matches!(
        config_with_allowed_metrics(&oversized),
        Err(AttestError::Config(message)) if message.contains("4096")
    ));

    let too_many = (0..65)
        .map(|index| format!("metric_{index}:0:1"))
        .collect::<Vec<_>>()
        .join(",");
    assert!(matches!(
        config_with_allowed_metrics(&too_many),
        Err(AttestError::Config(message)) if message.contains("64 entries")
    ));
}

#[test]
fn allowed_metrics_rejects_duplicates_empty_entries_and_invalid_names() {
    for invalid in [
        "temp_c:-40:85,temp_c:0:100",
        "temp_c:-40:85,,humidity:0:100",
        "temp c:-40:85",
    ] {
        assert!(
            matches!(
                config_with_allowed_metrics(invalid),
                Err(AttestError::Config(_))
            ),
            "invalid allowlist must fail closed: {invalid}"
        );
    }
}

// ── injection drills (first) ─────────────────────────────────────────────────

#[test]
fn smuggled_key_is_a_serde_error() {
    let raw = r#"{"kind":"reading","metric":"temp_c","value":4.2,"recipient":"EVIL"}"#;
    let parsed: Result<AttestArgs, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "unknown `recipient` field must fail deserialization"
    );
}

#[test]
fn caller_supplied_timestamp_is_rejected() {
    let raw = r#"{"kind":"reading","metric":"temp_c","value":4.2,"ts":1}"#;
    let parsed: Result<AttestArgs, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "timestamps must come from the trusted host clock"
    );
}

#[test]
fn metric_not_in_allowlist_rejected() {
    let r = execute_attest(&reading("evil_metric", 1.0), &cfg(), fresh_chain(), NOW);
    assert!(
        matches!(r, Err(AttestError::Rejected(_)) | Err(AttestError::Args(_))),
        "got {r:?}"
    );
}

#[test]
fn value_out_of_bounds_rejected() {
    let r = execute_attest(&reading("temp_c", 999.0), &cfg(), fresh_chain(), NOW);
    assert!(matches!(r, Err(AttestError::Rejected(_))), "got {r:?}");
    let r2 = execute_attest(&reading("temp_c", -100.0), &cfg(), fresh_chain(), NOW);
    assert!(matches!(r2, Err(AttestError::Rejected(_))), "got {r2:?}");
}

#[test]
fn value_nan_or_inf_rejected() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let r = execute_attest(&reading("temp_c", bad), &cfg(), fresh_chain(), NOW);
        assert!(
            matches!(r, Err(AttestError::Rejected(_))),
            "{bad} must be rejected, got {r:?}"
        );
    }
}

#[test]
fn nonce_is_snapshotted_before_chain_recovery() {
    let mock = fresh_chain();
    execute_attest(&reading("temp_c", 4.2), &cfg(), &mock, NOW).unwrap();
    let calls = mock.call_order.borrow();
    assert_eq!(
        calls.first(),
        Some(&"account"),
        "nonce must be read first: {calls:?}"
    );
    assert_eq!(
        calls.get(1),
        Some(&"signatures"),
        "history recovery follows nonce snapshot: {calls:?}"
    );
    assert_eq!(
        mock.signature_min_slots.borrow().as_slice(),
        &[Some(1)],
        "history request must be pinned to the nonce snapshot slot"
    );
}

// ── structural safety: funds cannot move ─────────────────────────────────────

#[test]
fn tx_contains_only_memo_and_system_programs() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    let mut progs = out.program_ids();
    progs.sort();
    let mut expected = vec![nonce::SYSTEM_PROGRAM_ID, memo::memo_program_id()];
    expected.sort();
    assert_eq!(
        progs, expected,
        "attestation tx must contain ONLY Memo + System programs"
    );
}

// ── Fix B: the fulfillment marker ────────────────────────────────────────────

#[test]
fn fulfillment_marker_carries_the_tag_and_the_charge_reference() {
    // kiosk-watch finds this marker by scanning getSignaturesForAddress on the
    // reference and grepping the memo for the tag, so both must be present.
    let out = execute_attest(&fulfillment(REFERENCE), &cfg(), fresh_chain(), NOW).unwrap();
    let memo_json = memo_text(&out);
    assert!(
        memo_json.contains("PKFUL1"),
        "memo must carry the fulfillment tag: {memo_json}"
    );
    assert!(
        memo_json.contains(REFERENCE),
        "memo must name the charge: {memo_json}"
    );
    let payment_signature = b58::encode(&[7u8; 64]);
    assert!(
        memo_json.contains(&payment_signature),
        "memo should record the payment it fulfilled: {memo_json}"
    );
}

#[test]
fn fulfillment_reference_is_an_account_key_so_the_marker_is_discoverable() {
    // A memo alone is invisible to getSignaturesForAddress(reference): the
    // reference has to be an account key of the transaction.
    let out = execute_attest(&fulfillment(REFERENCE), &cfg(), fresh_chain(), NOW).unwrap();
    let want = b58::decode_pubkey(REFERENCE).unwrap();
    assert!(
        out.message.account_keys.contains(&want),
        "the charge reference must appear in accountKeys"
    );
}

#[test]
fn fulfillment_reference_is_read_only_and_not_a_signer() {
    // The kiosk cannot produce a signature for the reference keypair, and the
    // marker must not need one. A signer-flagged reference would make the
    // transaction unsignable and the on-chain replay marker would never land.
    let out = execute_attest(&fulfillment(REFERENCE), &cfg(), fresh_chain(), NOW).unwrap();
    let want = b58::decode_pubkey(REFERENCE).unwrap();
    let idx = out
        .message
        .account_keys
        .iter()
        .position(|k| *k == want)
        .expect("reference is a key");
    assert!(
        idx >= out.message.header.num_required_signatures as usize,
        "reference must not be in the signer prefix of accountKeys"
    );
    let readonly_unsigned_start = out
        .message
        .account_keys
        .len()
        .checked_sub(out.message.header.num_readonly_unsigned_accounts as usize)
        .unwrap();
    assert!(
        idx >= readonly_unsigned_start,
        "reference must be in the readonly unsigned suffix of accountKeys"
    );
}

#[test]
fn fulfillment_reference_cannot_collide_with_transaction_infrastructure() {
    for reference in [
        NONCE_ACCOUNT,
        NONCE_AUTHORITY,
        nonce::SYSTEM_PROGRAM_ID_B58,
        nonce::RECENT_BLOCKHASHES_SYSVAR_B58,
        memo::MEMO_PROGRAM_ID_B58,
    ] {
        let mock = fresh_chain();
        let result = execute_attest(&fulfillment(reference), &cfg(), &mock, NOW);
        assert!(
            matches!(result, Err(AttestError::Args(ref message)) if message.contains("independent charge key")),
            "reserved reference {reference} was accepted: {result:?}"
        );
        assert_eq!(mock.account_calls.get(), 0, "must reject before RPC");
        assert_eq!(mock.sig_calls.get(), 0, "must reject before RPC");
    }
}

#[test]
fn fulfillment_tx_contains_only_memo_and_system_programs() {
    // The T1 invariant is unchanged by the new kind: a transfer is still not
    // expressible, so even a fully compromised model cannot turn a delivery
    // receipt into a spend.
    let out = execute_attest(&fulfillment(REFERENCE), &cfg(), fresh_chain(), NOW).unwrap();
    let mut progs = out.program_ids();
    progs.sort();
    let mut expected = vec![nonce::SYSTEM_PROGRAM_ID, memo::memo_program_id()];
    expected.sort();
    assert_eq!(
        progs, expected,
        "fulfillment tx must contain ONLY Memo + System programs"
    );
}

#[test]
fn fulfillment_memo_keeps_seq_so_the_attestation_chain_does_not_gap() {
    // The marker lands on the nonce account too, so it becomes the newest memo
    // there. chain::recover treats a newest memo without `seq` as a Gap, which
    // would break the NEXT attestation.
    let out = execute_attest(&fulfillment(REFERENCE), &cfg(), fresh_chain(), NOW).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&memo_text(&out)).expect("memo is JSON");
    assert!(parsed.get("seq").is_some(), "memo must carry seq");
    assert!(
        parsed.get("prev").is_some(),
        "memo must carry the prev link"
    );
}

#[test]
fn fulfillment_without_a_reference_is_rejected() {
    let args = AttestArgs {
        kind: Some("fulfillment".into()),
        ..Default::default()
    };
    let r = execute_attest(&args, &cfg(), fresh_chain(), NOW);
    assert!(matches!(r, Err(AttestError::Args(_))), "got {r:?}");
}

#[test]
fn fulfillment_with_a_malformed_reference_is_rejected() {
    let r = execute_attest(&fulfillment("not-a-pubkey"), &cfg(), fresh_chain(), NOW);
    assert!(matches!(r, Err(AttestError::Args(_))), "got {r:?}");
}

#[test]
fn fulfillment_requires_catalog_item_and_real_payment_signature() {
    let mut missing_item = fulfillment(REFERENCE);
    missing_item.item = None;
    assert!(matches!(
        execute_attest(&missing_item, &cfg(), fresh_chain(), NOW),
        Err(AttestError::Args(_))
    ));

    let mut missing_signature = fulfillment(REFERENCE);
    missing_signature.payment_sig = None;
    assert!(matches!(
        execute_attest(&missing_signature, &cfg(), fresh_chain(), NOW),
        Err(AttestError::Args(_))
    ));

    let mut malformed_signature = fulfillment(REFERENCE);
    malformed_signature.payment_sig = Some("not-a-signature".into());
    assert!(matches!(
        execute_attest(&malformed_signature, &cfg(), fresh_chain(), NOW),
        Err(AttestError::Args(_))
    ));
}

#[test]
fn advance_nonce_is_instruction_zero() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    let ix0 = &out.message.instructions[0];
    assert_eq!(
        out.message.account_keys[ix0.program_id_index as usize],
        nonce::SYSTEM_PROGRAM_ID
    );
    assert_eq!(
        ix0.data,
        vec![4, 0, 0, 0],
        "instruction 0 must be AdvanceNonceAccount"
    );
    assert_eq!(out.message.instructions.len(), 2);
}

#[test]
fn memo_instruction_present_and_carries_the_reading() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    let memo_id = memo::memo_program_id();
    let memo_ix = out
        .message
        .instructions
        .iter()
        .find(|ci| out.message.account_keys[ci.program_id_index as usize] == memo_id)
        .expect("memo instruction present");
    let json: serde_json::Value = serde_json::from_slice(&memo_ix.data).unwrap();
    assert_eq!(json["metric"], "temp_c");
    assert_eq!(json["dev"], "kiosk01");
}

#[test]
fn output_is_unsigned_zero_signatures() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    // The output is the bare serialized message — no signature section prepended.
    let decoded = b64::decode(&out.tx_base64).unwrap();
    assert_eq!(decoded, out.message.serialize());
    // First byte is the header's num_required_signatures (1), NOT a signature blob.
    assert_eq!(decoded[0], out.message.header.num_required_signatures);
}

#[test]
fn machine_output_preserves_the_complete_unsigned_message() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    let value: serde_json::Value = serde_json::from_str(&out.machine_output()).unwrap();
    assert_eq!(value["v"], 1);
    assert_eq!(value["success"], true);
    assert_eq!(value["status"], "signature_required");
    assert_eq!(value["unsigned_message_base64"], out.tx_base64);
    assert_eq!(
        b64::decode(value["unsigned_message_base64"].as_str().unwrap()).unwrap(),
        out.message.serialize()
    );
}

#[test]
fn memo_timestamp_is_the_host_clock_value() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    let memo: serde_json::Value = serde_json::from_str(&memo_text(&out)).unwrap();
    assert_eq!(memo["ts"], NOW);
}

#[test]
fn invalid_system_clock_fails_instead_of_becoming_epoch_zero() {
    let before_epoch = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        unix_timestamp(before_epoch),
        Err(AttestError::Clock(message)) if message.contains("before the Unix epoch")
    ));
    assert_eq!(unix_timestamp(UNIX_EPOCH).unwrap(), 0);
}

#[test]
fn seq_increments_and_prev_is_linked() {
    let out = execute_attest(
        &reading("temp_c", 4.2),
        &cfg(),
        chain_at_seq(7, "PrevSig9"),
        NOW,
    )
    .unwrap();
    assert_eq!(out.seq, 8);
    let memo_id = memo::memo_program_id();
    let memo_ix = out
        .message
        .instructions
        .iter()
        .find(|ci| out.message.account_keys[ci.program_id_index as usize] == memo_id)
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&memo_ix.data).unwrap();
    assert_eq!(json["seq"], 8);
    assert_eq!(json["prev"], "PrevSig9");
}

#[test]
fn fresh_device_starts_at_seq_zero_with_null_prev() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    assert_eq!(out.seq, 0);
    let memo_id = memo::memo_program_id();
    let memo_ix = out
        .message
        .instructions
        .iter()
        .find(|ci| out.message.account_keys[ci.program_id_index as usize] == memo_id)
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&memo_ix.data).unwrap();
    assert!(json["prev"].is_null());
}

// ── no secrets leak; output budget; config fail-closed ───────────────────────

#[test]
fn secrets_never_in_summary() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    assert!(
        !out.summary.contains(RPC),
        "rpc_url must not leak into output"
    );
    assert!(
        !out.summary.contains(NONCE_AUTHORITY),
        "authority must not leak into output"
    );
}

#[test]
fn fulfillment_summary_within_token_budget() {
    let out = execute_attest(&fulfillment(REFERENCE), &cfg(), fresh_chain(), NOW).unwrap();
    assert!(
        kiosk_core::shape::approx_tokens(&out.summary) <= kiosk_core::shape::DEFAULT_BUDGET_TOKENS
    );
}

#[test]
fn summary_within_token_budget() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    assert!(
        kiosk_core::shape::approx_tokens(&out.summary) <= kiosk_core::shape::DEFAULT_BUDGET_TOKENS
    );
}

#[test]
fn summary_never_claims_an_unsigned_message_landed_on_chain() {
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), fresh_chain(), NOW).unwrap();
    assert!(out.summary.starts_with("BUILT "));
    assert!(out.summary.contains("signature required"));
    assert!(!out.summary.contains("ATTESTED"));
    assert!(!out.summary.contains("FINALIZED"));
}

#[test]
fn unsupported_custody_mode_is_rejected() {
    let section: HashMap<String, String> = [
        ("rpc_url", RPC),
        ("device_id", "kiosk01"),
        ("nonce_account", NONCE_ACCOUNT),
        ("nonce_authority", NONCE_AUTHORITY),
        ("custody_mode", "t0"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert!(matches!(
        AttestConfig::from_section(&section),
        Err(AttestError::Config(_))
    ));
}

#[test]
fn oversized_model_text_is_rejected_before_rpc() {
    let args = AttestArgs {
        kind: Some("event".into()),
        event: Some("x".repeat(65)),
        ..Default::default()
    };
    let result = execute_attest(&args, &cfg(), fresh_chain(), NOW);
    assert!(
        matches!(result, Err(AttestError::Args(_))),
        "got {result:?}"
    );
}

#[test]
fn nonce_account_must_be_system_owned_and_non_executable() {
    let wrong_owner = account_info().replace(
        "11111111111111111111111111111111",
        "Vote111111111111111111111111111111111111111",
    );
    let owner_result = execute_attest(
        &reading("temp_c", 4.2),
        &cfg(),
        genesis_chain(wrong_owner),
        NOW,
    );
    assert!(matches!(owner_result, Err(AttestError::Decode(_))));

    let executable = account_info().replace("\"executable\":false", "\"executable\":true");
    let executable_result = execute_attest(
        &reading("temp_c", 4.2),
        &cfg(),
        genesis_chain(executable),
        NOW,
    );
    assert!(matches!(executable_result, Err(AttestError::Decode(_))));
}

#[test]
fn bad_nonce_pubkey_config_fails_closed() {
    let section: HashMap<String, String> = [
        ("rpc_url", RPC),
        ("device_id", "kiosk01"),
        ("nonce_account", "not-a-pubkey"),
        ("nonce_authority", NONCE_AUTHORITY),
        ("allowed_metrics", "temp_c:-40:85"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert!(matches!(
        AttestConfig::from_section(&section),
        Err(AttestError::Config(_))
    ));
}

#[test]
fn rpc_failure_is_never_a_successful_attestation() {
    let mock = Mock::build(Err(RpcError::Transport("down".into())), Ok(account_info()));
    let r: Result<AttestOutput, AttestError> =
        execute_attest(&reading("temp_c", 4.2), &cfg(), mock, NOW);
    assert!(r.is_err());
}

// ── USER-FRIENDLY + SECURE: human errors that leak no secrets ────────────────

#[test]
fn misconfig_errors_are_human_and_leak_no_rpc_url() {
    let sec = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    // Missing rpc_url → names the missing key.
    let e = AttestConfig::from_section(&sec(&[("device_id", "k")])).unwrap_err();
    assert!(e.to_string().contains("rpc_url"), "unhelpful: {e}");
    // Bad nonce pubkey → names the field and 'pubkey'; no rpc leak.
    let e2 = AttestConfig::from_section(&sec(&[
        ("rpc_url", RPC),
        ("device_id", "k"),
        ("nonce_account", "xx"),
        ("nonce_authority", NONCE_AUTHORITY),
    ]))
    .unwrap_err();
    let s = e2.to_string();
    assert!(
        s.contains("nonce_account") && s.contains("pubkey"),
        "unhelpful: {s}"
    );
    assert!(!s.contains(RPC), "rpc_url leaked into error: {s}");
}

// ── FAST: seq/prev recovery is exactly ONE getSignaturesForAddress ───────────

#[test]
fn recovers_chain_in_exactly_one_signatures_call() {
    let mock = fresh_chain();
    // Borrow via impl RpcTransport for &T so counters are readable afterward.
    let out = execute_attest(&reading("temp_c", 4.2), &cfg(), &mock, NOW).unwrap();
    assert!(out.seq == 0);
    assert_eq!(
        mock.sig_calls.get(),
        1,
        "chain recovery must be ONE getSignaturesForAddress"
    );
    assert_eq!(
        mock.account_calls.get(),
        1,
        "one getAccountInfo for the durable nonce"
    );
}
