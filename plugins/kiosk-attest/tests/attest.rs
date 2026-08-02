//! Host tests for the kiosk-attest core. RPC (chain recovery + nonce read) is
//! mocked; NO live network. Injection drills come first. The load-bearing test
//! is structural: the built transaction can contain ONLY the Memo and System
//! (advance-nonce) programs — a transfer is not expressible.

use std::collections::HashMap;

use kiosk_attest::attest::{execute_attest, AttestArgs, AttestConfig, AttestError, AttestOutput};
use kiosk_core::rpc::{RpcError, RpcTransport};
use kiosk_core::{b58, b64, memo, nonce};

const NONCE_AUTHORITY: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
const NONCE_ACCOUNT: &str = "So11111111111111111111111111111111111111112";
const RPC: &str = "https://api.devnet.solana.com";
const NOW: u64 = 1_700_000_000;

// ── mock: dispatch by method; chain sigs + nonce account info ────────────────

struct Mock {
    sigs: Result<String, RpcError>,
    account: Result<String, RpcError>,
    sig_calls: std::cell::Cell<u32>,
    account_calls: std::cell::Cell<u32>,
}
impl Mock {
    fn build(sigs: Result<String, RpcError>, account: Result<String, RpcError>) -> Self {
        Self {
            sigs,
            account,
            sig_calls: std::cell::Cell::new(0),
            account_calls: std::cell::Cell::new(0),
        }
    }
}
impl RpcTransport for Mock {
    fn send(&self, req: &str) -> Result<String, RpcError> {
        if req.contains("getSignaturesForAddress") {
            self.sig_calls.set(self.sig_calls.get() + 1);
            self.sigs.clone()
        } else if req.contains("getAccountInfo") {
            self.account_calls.set(self.account_calls.get() + 1);
            self.account.clone()
        } else {
            Err(RpcError::Transport("unexpected method".into()))
        }
    }
}
fn env(result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result}}}"#)
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
        r#"{{"context":{{"slot":1}},"value":{{"data":["{data}","base64"],"executable":false}}}}"#
    ))
}
fn fresh_chain() -> Mock {
    Mock::build(Ok(env("[]")), Ok(account_info()))
}
fn chain_at_seq(seq: u64, sig: &str) -> Mock {
    let sigs = env(&format!(
        r#"[{{"signature":"{sig}","slot":9,"memo":"[20] {{\"v\":1,\"seq\":{seq}}}"}}]"#
    ));
    Mock::build(Ok(sigs), Ok(account_info()))
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
        payment_sig: Some("5xPaymentSig".into()),
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
    assert!(
        memo_json.contains("5xPaymentSig"),
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
    // transaction unsignable and single-use would silently never work.
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
