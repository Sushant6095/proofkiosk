//! Authenticated attestation-chain recovery: derive the next
//! `(seq, prev_signature)` for a device address from a bounded finalized scan.
//!
//! A public nonce account can be named by anyone, so memo text alone is not a
//! chain head. We scan a fixed window and fetch every successful transaction.
//! Public traffic is ignored, but a transaction signed by the configured nonce
//! authority and naming the configured nonce account must be either the exact
//! nonce initialization or the exact `[advanceNonce, Memo]` attestation shape.
//! Anything else is a gap. Genesis is returned only when that initialization is
//! actually present in the scanned history; empty or pruned history fails
//! closed instead of silently resetting the sequence.

use serde_json::{json, Value};

use crate::rpc::{RpcClient, RpcError, RpcTransport};
use crate::{memo, nonce};

/// Public signature-list entries inspected in one recovery. This is larger
/// than the authenticated checkpoint depth so cheap outsider writes cannot
/// consume the whole proof window with one transaction.
const CHAIN_SCAN_LIMIT: u64 = 100;
/// Number of newest authenticated attestation links required before a mature
/// chain may use the oldest verified link as its bounded checkpoint.
const AUTHENTICATED_SUFFIX_DEPTH: usize = 10;
const NONCE_ACCOUNT_LEN: u64 = 80;
const RENT_SYSVAR_B58: &str = "SysvarRent111111111111111111111111111111111";

#[derive(Debug, PartialEq)]
pub struct ChainState {
    /// The seq the NEXT attestation should use.
    pub seq: u64,
    /// The previous landed signature this attestation should chain onto.
    pub prev_signature: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum ChainError {
    Rpc(String),
    Decode(String),
    /// The bounded history cannot prove an unbroken authenticated chain state.
    Gap(String),
}

impl core::fmt::Display for ChainError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ChainError::Rpc(m) => write!(f, "rpc error: {m}"),
            ChainError::Decode(m) => write!(f, "malformed rpc response: {m}"),
            ChainError::Gap(m) => write!(f, "attestation chain gap: {m}"),
        }
    }
}

impl From<RpcError> for ChainError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Transport(m) => ChainError::Rpc(m),
            RpcError::Rpc { code, message } => ChainError::Rpc(format!("{code}: {message}")),
            RpcError::Decode(m) => ChainError::Decode(m),
        }
    }
}

/// Recover the next authenticated chain state for `device`.
pub fn recover<T: RpcTransport>(
    device: &str,
    authority: &str,
    expected_device_id: &str,
    transport: T,
    finality: &str,
) -> Result<ChainState, ChainError> {
    recover_at_min_context_slot(
        device,
        authority,
        expected_device_id,
        transport,
        finality,
        None,
    )
}

/// Recover like [`recover`], but require the RPC node serving history to have
/// reached at least `min_context_slot`. Attestation uses the slot returned by
/// its preceding nonce-account snapshot so a lagging load-balanced backend
/// cannot pair an old chain head with a newer durable nonce.
pub fn recover_at_min_context_slot<T: RpcTransport>(
    device: &str,
    authority: &str,
    expected_device_id: &str,
    transport: T,
    finality: &str,
    min_context_slot: Option<u64>,
) -> Result<ChainState, ChainError> {
    let client = RpcClient::new(transport);
    let mut options = json!({ "commitment": finality, "limit": CHAIN_SCAN_LIMIT });
    if let Some(slot) = min_context_slot {
        options["minContextSlot"] = json!(slot);
    }
    let res = client.call("getSignaturesForAddress", json!([device, options]))?;
    let arr = res.as_array().ok_or_else(|| {
        ChainError::Decode("getSignaturesForAddress did not return an array".into())
    })?;

    if arr.len() > CHAIN_SCAN_LIMIT as usize {
        return Err(ChainError::Decode(format!(
            "getSignaturesForAddress exceeded the requested {CHAIN_SCAN_LIMIT}-entry limit"
        )));
    }

    // Validate the complete authenticated suffix visible in the bounded,
    // newest-first history. Every attestation must name the next older
    // authenticated transaction and decrement by exactly one. If sequence zero
    // is visible, exact nonce initialization must immediately follow it. For a
    // mature chain whose genesis is outside the window, acceptance requires a
    // fixed-depth unbroken authenticated suffix. Public traffic is skipped but
    // still consumes the larger total scan budget, so work remains bounded.
    let mut recovered: Option<ChainState> = None;
    let mut expected_previous: Option<(String, u64)> = None;
    let mut needs_initialization = false;
    // Exact initialization is only provisional genesis until the remainder of
    // the bounded public history proves there is no older authenticated device
    // history. The same nonce pubkey can be closed and recreated, so returning
    // immediately at a newer creation would otherwise permit a visible reset.
    let mut state_after_initialization: Option<ChainState> = None;
    let mut authenticated_attestations = 0usize;
    for entry in arr {
        let signature = entry
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| ChainError::Decode("signature entry missing `signature`".into()))?;
        let entry_err = entry
            .get("err")
            .ok_or_else(|| ChainError::Decode("signature entry missing `err`".into()))?;
        if !entry_err.is_null() {
            // A failed transaction did not change the nonce or chain state.
            if expected_previous
                .as_ref()
                .is_some_and(|(expected_signature, _)| expected_signature == signature)
            {
                return Err(ChainError::Gap(
                    "attestation prev identifies a failed transaction".into(),
                ));
            }
            continue;
        }
        let listed_memo = match entry.get("memo") {
            Some(Value::String(m)) => Some(m.as_str()),
            Some(Value::Null) => None,
            Some(_) => {
                return Err(ChainError::Decode(
                    "signature entry `memo` is neither a string nor null".into(),
                ))
            }
            None => return Err(ChainError::Decode("signature entry missing `memo`".into())),
        };

        let txv = client.call(
            "getTransaction",
            json!([signature, {
                "commitment": finality,
                "encoding": "jsonParsed",
                "maxSupportedTransactionVersion": 0
            }]),
        )?;
        let classified = classify_device_transaction(&txv, device, authority, listed_memo)?;

        if state_after_initialization.is_some() {
            match classified {
                None => continue,
                Some(DeviceTransaction::Initialization | DeviceTransaction::Attestation(_)) => {
                    return Err(ChainError::Gap(
                        "nonce initialization has older authenticated device history inside the bounded scan"
                            .into(),
                    ));
                }
            }
        }

        if needs_initialization {
            match classified {
                None => continue,
                Some(DeviceTransaction::Initialization) => {
                    let state = recovered.take().ok_or_else(|| {
                        ChainError::Gap("sequence-zero proof has no recoverable chain head".into())
                    })?;
                    needs_initialization = false;
                    state_after_initialization = Some(state);
                    continue;
                }
                Some(DeviceTransaction::Attestation(_)) => {
                    return Err(ChainError::Gap(
                        "sequence-zero attestation is not directly above nonce initialization"
                            .into(),
                    ));
                }
            }
        }

        if let Some((expected_signature, expected_seq)) = expected_previous.take() {
            match classified {
                // Public traffic cannot advance or fork the authenticated chain.
                None => {
                    expected_previous = Some((expected_signature, expected_seq));
                    continue;
                }
                Some(DeviceTransaction::Initialization) => {
                    return Err(ChainError::Gap(
                        "nonce initialization appears where an attestation prev is required".into(),
                    ));
                }
                Some(DeviceTransaction::Attestation(previous_memo)) => {
                    if signature != expected_signature {
                        return Err(ChainError::Gap(
                            "attestation prev skips the immediate older authenticated transaction"
                                .into(),
                        ));
                    }
                    let previous = parse_attestation_memo(&previous_memo, expected_device_id)?;
                    if previous.seq != expected_seq {
                        return Err(ChainError::Gap(
                            "attestation sequence does not continue from its prev signature".into(),
                        ));
                    }
                    authenticated_attestations += 1;
                    if previous.seq == 0 {
                        if previous.prev_signature.is_some() {
                            return Err(ChainError::Gap(
                                "sequence-zero attestation must have prev=null".into(),
                            ));
                        }
                        needs_initialization = true;
                    } else {
                        let prior_signature = previous.prev_signature.ok_or_else(|| {
                            ChainError::Gap("nonzero attestation has no previous signature".into())
                        })?;
                        if authenticated_attestations == AUTHENTICATED_SUFFIX_DEPTH {
                            return recovered.ok_or_else(|| {
                                ChainError::Gap(
                                    "authenticated suffix has no recoverable chain head".into(),
                                )
                            });
                        }
                        expected_previous = Some((prior_signature, previous.seq - 1));
                    }
                    continue;
                }
            }
        }

        match classified {
            // Anyone can mention the public nonce account. Only authority-signed
            // traffic is allowed to affect (or break) the recovered chain.
            None => continue,
            Some(DeviceTransaction::Initialization) => {
                state_after_initialization = Some(ChainState {
                    seq: 0,
                    prev_signature: None,
                });
                continue;
            }
            Some(DeviceTransaction::Attestation(memo_text)) => {
                let head = parse_attestation_memo(&memo_text, expected_device_id)?;
                let seq = head
                    .seq
                    .checked_add(1)
                    .ok_or_else(|| ChainError::Gap("attestation sequence overflow".into()))?;
                recovered = Some(ChainState {
                    seq,
                    prev_signature: Some(signature.to_string()),
                });
                authenticated_attestations = 1;
                if head.seq == 0 {
                    if head.prev_signature.is_some() {
                        return Err(ChainError::Gap(
                            "sequence-zero attestation must have prev=null".into(),
                        ));
                    }
                    needs_initialization = true;
                    continue;
                }

                let previous_signature = head.prev_signature.ok_or_else(|| {
                    ChainError::Gap("nonzero attestation has no previous signature".into())
                })?;
                expected_previous = Some((previous_signature, head.seq - 1));
            }
        }
    }

    if needs_initialization {
        return Err(ChainError::Gap(
            "sequence-zero attestation is present but nonce initialization is outside the bounded history"
                .into(),
        ));
    }

    if let Some(state) = state_after_initialization {
        if arr.len() == CHAIN_SCAN_LIMIT as usize {
            return Err(ChainError::Gap(format!(
                "nonce initialization is only provisional because the newest {CHAIN_SCAN_LIMIT}-transaction history window is truncated"
            )));
        }
        return Ok(state);
    }

    if recovered.is_some() {
        return Err(ChainError::Gap(
            "attestation prev is not proven inside the bounded public history scan".into(),
        ));
    }

    Err(ChainError::Gap(if arr.is_empty() {
        "device history is empty; nonce initialization cannot be proven".into()
    } else if arr.len() == CHAIN_SCAN_LIMIT as usize {
        format!(
            "no authenticated chain head or nonce initialization in the newest {CHAIN_SCAN_LIMIT} transactions"
        )
    } else {
        "scanned history has no authenticated chain head or nonce initialization".into()
    }))
}

struct ParsedAttestation {
    seq: u64,
    ts: u64,
    prev_signature: Option<String>,
}

/// Validate the exact memo schema emitted by `kiosk-attest` and return its
/// signed host-observation timestamp. The caller must first authenticate the
/// transaction envelope; this function validates memo content only.
pub fn attestation_timestamp(memo_text: &str, expected_device_id: &str) -> Result<u64, ChainError> {
    parse_attestation_memo(memo_text, expected_device_id).map(|memo| memo.ts)
}

fn parse_attestation_memo(
    memo_text: &str,
    expected_device_id: &str,
) -> Result<ParsedAttestation, ChainError> {
    let parsed: Value = serde_json::from_str(memo_text)
        .map_err(|_| ChainError::Gap("authenticated attestation memo is not JSON".into()))?;
    if parsed.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(ChainError::Gap(
            "authenticated attestation memo has unsupported version".into(),
        ));
    }
    if parsed.get("dev").and_then(Value::as_str) != Some(expected_device_id) {
        return Err(ChainError::Gap(
            "authenticated attestation memo has the wrong device id".into(),
        ));
    }
    let seq = parsed
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| ChainError::Gap("authenticated memo has no seq field".into()))?;
    let ts = parsed.get("ts").and_then(Value::as_u64).ok_or_else(|| {
        ChainError::Gap("authenticated attestation memo has no valid ts field".into())
    })?;
    let prev_signature = match parsed.get("prev") {
        Some(Value::Null) => None,
        Some(Value::String(signature)) if !signature.is_empty() => Some(signature.clone()),
        _ => {
            return Err(ChainError::Gap(
                "authenticated memo has an invalid prev field".into(),
            ))
        }
    };
    if !has_exact_attestation_body(&parsed) {
        return Err(ChainError::Gap(
            "authenticated attestation memo does not match an exact v1 reading, event, or fulfillment schema"
                .into(),
        ));
    }
    Ok(ParsedAttestation {
        seq,
        ts,
        prev_signature,
    })
}

fn has_exact_attestation_body(parsed: &Value) -> bool {
    let Some(object) = parsed.as_object() else {
        return false;
    };
    let has_exact_keys = |allowed: &[&str]| {
        object.len() == allowed.len() && object.keys().all(|key| allowed.contains(&key.as_str()))
    };
    let valid_text = |key: &str, max_bytes: usize| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty() && text.len() <= max_bytes)
    };

    if has_exact_keys(&["v", "dev", "seq", "ts", "prev", "metric", "val"]) {
        return valid_text("metric", 64)
            && object
                .get("metric")
                .and_then(Value::as_str)
                .is_some_and(|metric| {
                    metric.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                    })
                })
            && object
                .get("val")
                .and_then(Value::as_f64)
                .is_some_and(f64::is_finite);
    }

    let event_keys = [
        "v",
        "dev",
        "seq",
        "ts",
        "prev",
        "event",
        "item",
        "payment_sig",
    ];
    if object.contains_key("event")
        && object.keys().all(|key| event_keys.contains(&key.as_str()))
        && object.len()
            == 6 + usize::from(object.contains_key("item"))
                + usize::from(object.contains_key("payment_sig"))
    {
        return valid_text("event", 64)
            && (!object.contains_key("item") || valid_text("item", 64))
            && (!object.contains_key("payment_sig") || valid_text("payment_sig", 128));
    }

    if has_exact_keys(&[
        "v",
        "dev",
        "seq",
        "ts",
        "prev",
        "tag",
        "ref",
        "item",
        "payment_sig",
    ]) {
        return object.get("tag").and_then(Value::as_str) == Some(memo::FULFILLMENT_TAG)
            && object
                .get("ref")
                .and_then(Value::as_str)
                .and_then(crate::b58::decode_pubkey)
                .is_some()
            && valid_text("item", 64)
            && object
                .get("payment_sig")
                .and_then(Value::as_str)
                .filter(|signature| signature.len() <= 128)
                .and_then(crate::b58::decode)
                .is_some_and(|signature| signature.len() == 64);
    }

    false
}

/// The RPC `memo` field may be prefixed (e.g. `"[31] {...}"`). Return the
/// substring from the first `{` — our attestation memos are JSON objects.
fn extract_json(memo: &str) -> Option<&str> {
    memo.find('{').map(|i| &memo[i..])
}

#[derive(Debug, PartialEq)]
enum DeviceTransaction {
    Initialization,
    Attestation(String),
}

/// Identify the one authority-signed, memo-less device transaction that is not
/// an attestation: exact nonce-account creation + initialization. Callers such
/// as heartbeat scanners may ignore this genesis record while still failing
/// closed on every other authority-signed memo-less device mutation.
pub fn is_exact_nonce_initialization_transaction(
    txv: &Value,
    device: &str,
    authority: &str,
) -> Result<bool, ChainError> {
    let Some(parts) = authenticated_device_parts(txv, device, authority)? else {
        return Ok(false);
    };
    Ok(is_exact_initialization(
        parts.instructions,
        parts.keys,
        device,
        authority,
        None,
    ))
}

/// Classify one successful transaction returned for the public device address.
///
/// `Ok(None)` means unauthenticated public traffic and is safe to ignore.
/// Once the configured authority signature and device account are both present,
/// every structural mismatch is a chain gap: an authorized write must never be
/// silently skipped in favor of an older head.
fn classify_device_transaction(
    txv: &Value,
    device: &str,
    authority: &str,
    listed_memo: Option<&str>,
) -> Result<Option<DeviceTransaction>, ChainError> {
    let Some(parts) = authenticated_device_parts(txv, device, authority)? else {
        return Ok(None);
    };
    if is_exact_initialization(
        parts.instructions,
        parts.keys,
        device,
        authority,
        listed_memo,
    ) {
        return Ok(Some(DeviceTransaction::Initialization));
    }
    if let Some(memo_text) = exact_attestation_memo(
        parts.instructions,
        parts.keys,
        device,
        authority,
        listed_memo,
    ) {
        validate_fulfillment_reference_account(memo_text, parts.keys, device, authority)?;
        return Ok(Some(DeviceTransaction::Attestation(memo_text.to_string())));
    }

    Err(ChainError::Gap(
        "authority-signed device transaction is neither exact nonce initialization nor exact advanceNonce+Memo attestation"
            .into(),
    ))
}

struct AuthenticatedDeviceParts<'a> {
    keys: &'a [Value],
    instructions: &'a [Value],
}

fn authenticated_device_parts<'a>(
    txv: &'a Value,
    device: &str,
    authority: &str,
) -> Result<Option<AuthenticatedDeviceParts<'a>>, ChainError> {
    if txv.is_null() {
        return Err(ChainError::Gap(
            "a listed transaction is unavailable (history may be pruned)".into(),
        ));
    }
    let succeeded = txv
        .get("meta")
        .and_then(|m| m.get("err"))
        .map(Value::is_null)
        .ok_or_else(|| ChainError::Decode("getTransaction is missing meta.err".into()))?;
    if !succeeded {
        return Err(ChainError::Gap(
            "signature listing and transaction disagree about execution success".into(),
        ));
    }
    let message = txv
        .get("transaction")
        .and_then(|t| t.get("message"))
        .ok_or_else(|| ChainError::Decode("getTransaction is missing its message".into()))?;
    let keys = message
        .get("accountKeys")
        .and_then(Value::as_array)
        .ok_or_else(|| ChainError::Decode("transaction has no accountKeys".into()))?;
    validate_account_keys(keys)?;
    let names_device = keys
        .iter()
        .any(|key| account_key_pubkey(key) == Some(device));
    let authority_signed = keys.iter().any(|key| {
        account_key_pubkey(key) == Some(authority)
            && key.get("signer").and_then(Value::as_bool) == Some(true)
    });
    if !names_device || !authority_signed {
        return Ok(None);
    }

    let instructions = message
        .get("instructions")
        .and_then(Value::as_array)
        .ok_or_else(|| ChainError::Decode("transaction has no parsed instructions".into()))?;
    Ok(Some(AuthenticatedDeviceParts { keys, instructions }))
}

/// A fulfillment memo is only a usable cross-host replay marker when its
/// charge reference is actually attached to the Solana transaction. Requiring
/// one read-only non-signer occurrence preserves the discoverability contract
/// consumed by `kiosk-watch`'s reference-address scan.
fn validate_fulfillment_reference_account(
    memo_text: &str,
    keys: &[Value],
    device: &str,
    authority: &str,
) -> Result<(), ChainError> {
    let parsed: Value = serde_json::from_str(memo_text)
        .map_err(|_| ChainError::Gap("authenticated attestation memo is not JSON".into()))?;
    if parsed.get("tag").and_then(Value::as_str) != Some(memo::FULFILLMENT_TAG) {
        return Ok(());
    }
    let reference = parsed.get("ref").and_then(Value::as_str).ok_or_else(|| {
        ChainError::Gap("authenticated fulfillment memo has an invalid reference".into())
    })?;
    let reference_key = crate::b58::decode_pubkey(reference).ok_or_else(|| {
        ChainError::Gap("authenticated fulfillment memo has an invalid reference".into())
    })?;
    let device_key = crate::b58::decode_pubkey(device)
        .ok_or_else(|| ChainError::Gap("configured device pubkey is invalid".into()))?;
    let authority_key = crate::b58::decode_pubkey(authority)
        .ok_or_else(|| ChainError::Gap("configured authority pubkey is invalid".into()))?;
    if attestation_reference_collides(&reference_key, &device_key, &authority_key) {
        return Err(ChainError::Gap(
            "authenticated fulfillment reference collides with attestation transaction infrastructure"
                .into(),
        ));
    }
    let matching: Vec<&Value> = keys
        .iter()
        .filter(|key| account_key_pubkey(key) == Some(reference))
        .collect();
    if matching.len() != 1
        || matching[0].get("signer").and_then(Value::as_bool) != Some(false)
        || matching[0].get("writable").and_then(Value::as_bool) != Some(false)
    {
        return Err(ChainError::Gap(
            "authenticated fulfillment transaction does not bind exactly one read-only non-signer reference account"
                .into(),
        ));
    }
    Ok(())
}

/// Whether a fulfillment reference would be deduplicated with a fixed key in
/// the durable-nonce attestation transaction. Such a collision either upgrades
/// signer/writable flags or points discovery at a globally crowded program or
/// sysvar address, so producer and verifier must reject the same set.
pub fn attestation_reference_collides(
    reference: &[u8; 32],
    device: &[u8; 32],
    authority: &[u8; 32],
) -> bool {
    let recent_blockhashes = crate::b58::decode_pubkey(nonce::RECENT_BLOCKHASHES_SYSVAR_B58)
        .expect("recent blockhashes sysvar id is valid");
    [
        *device,
        *authority,
        nonce::SYSTEM_PROGRAM_ID,
        recent_blockhashes,
        memo::memo_program_id(),
    ]
    .contains(reference)
}

fn account_key_pubkey(value: &Value) -> Option<&str> {
    value.get("pubkey").and_then(Value::as_str)
}

fn validate_account_keys(keys: &[Value]) -> Result<(), ChainError> {
    if keys.is_empty() {
        return Err(ChainError::Decode("transaction has no account keys".into()));
    }
    for key in keys {
        if account_key_pubkey(key).is_none()
            || key.get("signer").and_then(Value::as_bool).is_none()
            || key.get("writable").and_then(Value::as_bool).is_none()
        {
            return Err(ChainError::Decode(
                "jsonParsed account key is missing pubkey/signer/writable".into(),
            ));
        }
    }
    Ok(())
}

fn account_flags(keys: &[Value], pubkey: &str) -> Option<(bool, bool)> {
    let key = keys
        .iter()
        .find(|key| account_key_pubkey(key) == Some(pubkey))?;
    Some((
        key.get("signer")?.as_bool()?,
        key.get("writable")?.as_bool()?,
    ))
}

fn is_system_instruction<'a>(ix: &'a Value, instruction_type: &str) -> Option<&'a Value> {
    if ix.get("program").and_then(Value::as_str) != Some("system")
        || ix.get("programId").and_then(Value::as_str) != Some(nonce::SYSTEM_PROGRAM_ID_B58)
        || ix
            .get("parsed")
            .and_then(|parsed| parsed.get("type"))
            .and_then(Value::as_str)
            != Some(instruction_type)
    {
        return None;
    }
    ix.get("parsed")?.get("info")
}

fn info_str<'a>(info: &'a Value, name: &str) -> Option<&'a str> {
    info.get(name).and_then(Value::as_str)
}

fn is_exact_initialization(
    instructions: &[Value],
    keys: &[Value],
    device: &str,
    authority: &str,
    listed_memo: Option<&str>,
) -> bool {
    if listed_memo.is_some()
        || instructions.len() != 2
        || account_flags(keys, authority).map(|f| f.0) != Some(true)
        || account_flags(keys, device) != Some((true, true))
    {
        return false;
    }

    let Some(create) = is_system_instruction(&instructions[0], "createAccount") else {
        return false;
    };
    let Some(initialize) = is_system_instruction(&instructions[1], "initializeNonce") else {
        return false;
    };
    info_str(create, "source") == Some(authority)
        && info_str(create, "newAccount") == Some(device)
        && info_str(create, "owner") == Some(nonce::SYSTEM_PROGRAM_ID_B58)
        && create.get("space").and_then(Value::as_u64) == Some(NONCE_ACCOUNT_LEN)
        && create
            .get("lamports")
            .and_then(Value::as_u64)
            .is_some_and(|lamports| lamports > 0)
        && info_str(initialize, "nonceAccount") == Some(device)
        && info_str(initialize, "nonceAuthority") == Some(authority)
        && info_str(initialize, "recentBlockhashesSysvar")
            == Some(nonce::RECENT_BLOCKHASHES_SYSVAR_B58)
        && info_str(initialize, "rentSysvar") == Some(RENT_SYSVAR_B58)
}

fn exact_attestation_memo<'a>(
    instructions: &'a [Value],
    keys: &[Value],
    device: &str,
    authority: &str,
    listed_memo: Option<&str>,
) -> Option<&'a str> {
    if instructions.len() != 2
        || account_flags(keys, authority).map(|f| f.0) != Some(true)
        || account_flags(keys, device) != Some((false, true))
    {
        return None;
    }

    let advance = is_system_instruction(&instructions[0], "advanceNonce")?;
    if info_str(advance, "nonceAccount") != Some(device)
        || info_str(advance, "nonceAuthority") != Some(authority)
        || info_str(advance, "recentBlockhashesSysvar")
            != Some(nonce::RECENT_BLOCKHASHES_SYSVAR_B58)
    {
        return None;
    }

    let memo_ix = &instructions[1];
    if memo_ix.get("program").and_then(Value::as_str) != Some("spl-memo")
        || memo_ix.get("programId").and_then(Value::as_str) != Some(memo::MEMO_PROGRAM_ID_B58)
    {
        return None;
    }
    let transaction_memo = memo_ix.get("parsed").and_then(Value::as_str)?;
    match listed_memo {
        Some(listed) => {
            let listed_payload = extract_json(listed)?;
            (transaction_memo == listed_payload).then_some(transaction_memo)
        }
        None => Some(transaction_memo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const DEVICE: &str = "So11111111111111111111111111111111111111112";
    const AUTHORITY: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
    const OUTSIDER: &str = "Vote111111111111111111111111111111111111111";

    struct Mock {
        sigs: Result<String, RpcError>,
        transactions: HashMap<String, String>,
    }

    impl RpcTransport for Mock {
        fn send(&self, req: &str) -> Result<String, RpcError> {
            let request: Value = serde_json::from_str(req)
                .map_err(|e| RpcError::Decode(format!("bad test request: {e}")))?;
            match request.get("method").and_then(Value::as_str) {
                Some("getSignaturesForAddress") => self.sigs.clone(),
                Some("getTransaction") => {
                    let signature = request
                        .get("params")
                        .and_then(|p| p.get(0))
                        .and_then(Value::as_str)
                        .ok_or_else(|| RpcError::Decode("missing test signature".into()))?;
                    self.transactions.get(signature).cloned().ok_or_else(|| {
                        RpcError::Transport(format!("no test transaction for {signature}"))
                    })
                }
                _ => Err(RpcError::Transport("unexpected test method".into())),
            }
        }
    }

    fn envelope(result: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": 1, "result": result }).to_string()
    }

    fn mock(entries: Value, txs: Vec<(&str, Value)>) -> Mock {
        Mock {
            sigs: Ok(envelope(entries)),
            transactions: txs
                .into_iter()
                .map(|(signature, tx)| (signature.to_string(), envelope(tx)))
                .collect(),
        }
    }

    fn key(pubkey: &str, signer: bool, writable: bool) -> Value {
        json!({ "pubkey": pubkey, "signer": signer, "writable": writable })
    }

    fn system_ix(kind: &str, info: Value) -> Value {
        json!({
            "program": "system",
            "programId": nonce::SYSTEM_PROGRAM_ID_B58,
            "parsed": { "type": kind, "info": info }
        })
    }

    fn memo_ix(text: &str) -> Value {
        json!({
            "program": "spl-memo",
            "programId": memo::MEMO_PROGRAM_ID_B58,
            "parsed": text
        })
    }

    fn transaction(signer: &str, device_signer: bool, instructions: Vec<Value>) -> Value {
        json!({
            "meta": { "err": null },
            "transaction": {
                "message": {
                    "accountKeys": [
                        key(signer, true, true),
                        key(DEVICE, device_signer, true),
                    ],
                    "instructions": instructions,
                }
            }
        })
    }

    fn attestation_tx(signer: &str, memo_text: &str) -> Value {
        transaction(
            signer,
            false,
            vec![
                system_ix(
                    "advanceNonce",
                    json!({
                        "nonceAccount": DEVICE,
                        "recentBlockhashesSysvar": nonce::RECENT_BLOCKHASHES_SYSVAR_B58,
                        "nonceAuthority": AUTHORITY,
                    }),
                ),
                memo_ix(memo_text),
            ],
        )
    }

    fn fulfillment_tx(signer: &str, memo_text: &str, reference: &str) -> Value {
        let mut tx = attestation_tx(signer, memo_text);
        tx["transaction"]["message"]["accountKeys"]
            .as_array_mut()
            .unwrap()
            .push(key(reference, false, false));
        tx
    }

    fn initialization_tx() -> Value {
        transaction(
            AUTHORITY,
            true,
            vec![
                system_ix(
                    "createAccount",
                    json!({
                        "source": AUTHORITY,
                        "newAccount": DEVICE,
                        "lamports": 1_500_000,
                        "space": NONCE_ACCOUNT_LEN,
                        "owner": nonce::SYSTEM_PROGRAM_ID_B58,
                    }),
                ),
                system_ix(
                    "initializeNonce",
                    json!({
                        "nonceAccount": DEVICE,
                        "recentBlockhashesSysvar": nonce::RECENT_BLOCKHASHES_SYSVAR_B58,
                        "rentSysvar": RENT_SYSVAR_B58,
                        "nonceAuthority": AUTHORITY,
                    }),
                ),
            ],
        )
    }

    fn public_junk_tx() -> Value {
        transaction(
            OUTSIDER,
            false,
            vec![system_ix(
                "transfer",
                json!({ "source": OUTSIDER, "destination": DEVICE, "lamports": 1 }),
            )],
        )
    }

    fn entry(signature: &str, memo: Option<&str>) -> Value {
        json!({ "signature": signature, "slot": 100, "err": null, "memo": memo })
    }

    fn reading_memo_for(device_id: &str, seq: u64, prev: Option<&str>) -> String {
        json!({
            "v": 1,
            "dev": device_id,
            "seq": seq,
            "ts": 1_000_000,
            "metric": "temp_c",
            "val": 4.2,
            "prev": prev,
        })
        .to_string()
    }

    fn reading_memo(seq: u64, prev: Option<&str>) -> String {
        reading_memo_for("k01", seq, prev)
    }

    fn authenticated_chain(head_seq: u64, prefix: &str, prefix_head_memo: bool) -> (String, Mock) {
        let head_signature = format!("{prefix}{head_seq}");
        let mut entries = Vec::new();
        let mut transactions = HashMap::new();
        let mut seq = head_seq;
        loop {
            let signature = format!("{prefix}{seq}");
            let prev = (seq > 0).then(|| format!("{prefix}{}", seq - 1));
            let memo = reading_memo(seq, prev.as_deref());
            let listed = if seq == head_seq && prefix_head_memo {
                format!("[{}] {memo}", memo.len())
            } else {
                memo.clone()
            };
            entries.push(entry(&signature, Some(&listed)));
            transactions.insert(signature, envelope(attestation_tx(AUTHORITY, &memo)));
            if entries.len() == AUTHENTICATED_SUFFIX_DEPTH {
                break;
            }
            if seq == 0 {
                entries.push(entry("InitSig", None));
                transactions.insert("InitSig".into(), envelope(initialization_tx()));
                break;
            }
            seq -= 1;
        }
        (
            head_signature,
            Mock {
                sigs: Ok(envelope(Value::Array(entries))),
                transactions,
            },
        )
    }

    #[test]
    fn nonce_creation_history_bootstraps_as_genesis() {
        let st = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("InitSig", None)]),
                vec![("InitSig", initialization_tx())],
            ),
            "finalized",
        )
        .unwrap();
        assert_eq!(
            st,
            ChainState {
                seq: 0,
                prev_signature: None
            }
        );
    }

    #[test]
    fn existing_chain_increments_seq_and_links_prev() {
        let (sig, history) = authenticated_chain(7, "Chain", true);
        let st = recover(DEVICE, AUTHORITY, "k01", history, "finalized").unwrap();
        assert_eq!(st.seq, 8);
        assert_eq!(st.prev_signature.as_deref(), Some(sig.as_str()));
    }

    #[test]
    fn memo_without_length_prefix_also_parses() {
        let (_, history) = authenticated_chain(41, "Long", false);
        let st = recover(DEVICE, AUTHORITY, "k01", history, "finalized").unwrap();
        assert_eq!(st.seq, 42);
    }

    #[test]
    fn public_junk_does_not_freeze_a_mature_authenticated_chain() {
        let (head_signature, mut history) = authenticated_chain(41, "Mature", false);
        let mut response: Value =
            serde_json::from_str(history.sigs.as_ref().expect("mock signature response")).unwrap();
        response["result"]
            .as_array_mut()
            .unwrap()
            .insert(0, entry("PublicJunk", None));
        history.sigs = Ok(response.to_string());
        history
            .transactions
            .insert("PublicJunk".into(), envelope(public_junk_tx()));

        let state = recover(DEVICE, AUTHORITY, "k01", history, "finalized").unwrap();
        assert_eq!(state.seq, 42);
        assert_eq!(
            state.prev_signature.as_deref(),
            Some(head_signature.as_str())
        );
    }

    #[test]
    fn head_cannot_skip_a_newer_authenticated_predecessor() {
        let head = reading_memo(8, Some("NamedPrev"));
        let fork = reading_memo(7, Some("Older"));
        let named = reading_memo(7, Some("Older"));
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([
                    entry("Head", Some(&head)),
                    entry("Fork", Some(&fork)),
                    entry("NamedPrev", Some(&named)),
                ]),
                vec![
                    ("Head", attestation_tx(AUTHORITY, &head)),
                    ("Fork", attestation_tx(AUTHORITY, &fork)),
                    ("NamedPrev", attestation_tx(AUTHORITY, &named)),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(
            matches!(err, ChainError::Gap(ref message) if message.contains("skips the immediate older")),
            "got {err:?}"
        );
    }

    #[test]
    fn authority_cannot_reset_existing_chain_to_sequence_zero() {
        let reset = reading_memo(0, None);
        let older = reading_memo(7, Some("Prior"));
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([
                    entry("Reset", Some(&reset)),
                    entry("Existing", Some(&older)),
                    entry("Init", None),
                ]),
                vec![
                    ("Reset", attestation_tx(AUTHORITY, &reset)),
                    ("Existing", attestation_tx(AUTHORITY, &older)),
                    ("Init", initialization_tx()),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn linked_head_cannot_hide_a_sequence_zero_reset() {
        let head = reading_memo(1, Some("Reset"));
        let reset = reading_memo(0, None);
        let existing = reading_memo(7, Some("Prior"));
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([
                    entry("Head", Some(&head)),
                    entry("Reset", Some(&reset)),
                    entry("Existing", Some(&existing)),
                    entry("Init", None),
                ]),
                vec![
                    ("Head", attestation_tx(AUTHORITY, &head)),
                    ("Reset", attestation_tx(AUTHORITY, &reset)),
                    ("Existing", attestation_tx(AUTHORITY, &existing)),
                    ("Init", initialization_tx()),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn recreated_nonce_initialization_cannot_hide_older_chain() {
        let older = reading_memo(7, Some("Prior"));
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("NewInit", None), entry("Older", Some(&older)),]),
                vec![
                    ("NewInit", initialization_tx()),
                    ("Older", attestation_tx(AUTHORITY, &older)),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(
            matches!(err, ChainError::Gap(ref message) if message.contains("older authenticated")),
            "a recreated nonce pubkey cannot reset visible history: {err:?}"
        );
    }

    #[test]
    fn provisional_initialization_at_full_scan_limit_is_a_gap() {
        let mut entries = vec![entry("NewInit", None)];
        let mut transactions =
            HashMap::from([("NewInit".to_string(), envelope(initialization_tx()))]);
        for index in 0..(CHAIN_SCAN_LIMIT - 1) {
            let signature = format!("Public{index}");
            entries.push(entry(&signature, None));
            transactions.insert(signature, envelope(public_junk_tx()));
        }
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            Mock {
                sigs: Ok(envelope(Value::Array(entries))),
                transactions,
            },
            "finalized",
        )
        .unwrap_err();
        assert!(
            matches!(err, ChainError::Gap(ref message) if message.contains("truncated")),
            "full-window provisional genesis must fail closed: {err:?}"
        );
    }

    #[test]
    fn sequence_zero_and_recreated_nonce_cannot_hide_older_chain() {
        let reset = reading_memo(0, None);
        let older = reading_memo(7, Some("Prior"));
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([
                    entry("Reset", Some(&reset)),
                    entry("NewInit", None),
                    entry("Older", Some(&older)),
                ]),
                vec![
                    ("Reset", attestation_tx(AUTHORITY, &reset)),
                    ("NewInit", initialization_tx()),
                    ("Older", attestation_tx(AUTHORITY, &older)),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(
            matches!(err, ChainError::Gap(ref message) if message.contains("older authenticated")),
            "sequence zero plus nonce recreation cannot reset visible history: {err:?}"
        );
    }

    #[test]
    fn complete_v1_attestation_body_schema_is_required() {
        let invalid = [
            json!({ "v":1, "dev":"k01", "seq":7, "prev":"Prior" }),
            json!({
                "v":1, "dev":"k01", "seq":7, "ts":1_000_000,
                "prev":"Prior", "metric":"temp_c", "val":4.2,
                "event":"door_open"
            }),
            json!({
                "v":1, "dev":"k01", "seq":7, "ts":1_000_000,
                "prev":"Prior", "event":"door_open", "unknown":true
            }),
            json!({
                "v":1, "dev":"k01", "seq":7, "ts":1_000_000,
                "prev":"Prior", "tag":memo::FULFILLMENT_TAG,
                "ref":DEVICE, "item":"cold_drink"
            }),
        ];
        for memo in invalid {
            let result = parse_attestation_memo(&memo.to_string(), "k01");
            assert!(result.is_err(), "invalid body was accepted: {memo}");
        }
    }

    #[test]
    fn every_emitted_v1_body_variant_is_chainable() {
        let valid = [
            serde_json::from_str::<Value>(&reading_memo(7, Some("Prior"))).unwrap(),
            json!({
                "v":1, "dev":"k01", "seq":7, "ts":1_000_000,
                "prev":"Prior", "event":"door_open", "item":"cold_drink",
                "payment_sig":"operator-receipt"
            }),
            json!({
                "v":1, "dev":"k01", "seq":7, "ts":1_000_000,
                "prev":"Prior", "tag":memo::FULFILLMENT_TAG,
                "ref":DEVICE, "item":"cold_drink",
                "payment_sig":"1111111111111111111111111111111111111111111111111111111111111111"
            }),
        ];
        for memo in valid {
            let parsed = parse_attestation_memo(&memo.to_string(), "k01")
                .unwrap_or_else(|error| panic!("emitted schema was rejected: {memo}: {error}"));
            assert_eq!(parsed.seq, 7);
            assert_eq!(parsed.prev_signature.as_deref(), Some("Prior"));
        }
    }

    #[test]
    fn fulfillment_without_reference_account_is_a_chain_gap() {
        let fulfillment = json!({
            "v":1, "dev":"k01", "seq":0, "ts":1_000_000,
            "prev":null, "tag":memo::FULFILLMENT_TAG,
            "ref":OUTSIDER, "item":"cold_drink",
            "payment_sig":"1111111111111111111111111111111111111111111111111111111111111111"
        })
        .to_string();
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Marker", Some(&fulfillment)), entry("Init", None),]),
                vec![
                    ("Marker", attestation_tx(AUTHORITY, &fulfillment)),
                    ("Init", initialization_tx()),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(
            matches!(err, ChainError::Gap(ref message) if message.contains("reference account")),
            "an undiscoverable fulfillment marker cannot become a chain link: {err:?}"
        );
    }

    #[test]
    fn fulfillment_with_readonly_reference_account_is_chainable() {
        let fulfillment = json!({
            "v":1, "dev":"k01", "seq":0, "ts":1_000_000,
            "prev":null, "tag":memo::FULFILLMENT_TAG,
            "ref":OUTSIDER, "item":"cold_drink",
            "payment_sig":"1111111111111111111111111111111111111111111111111111111111111111"
        })
        .to_string();
        let state = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Marker", Some(&fulfillment)), entry("Init", None),]),
                vec![
                    ("Marker", fulfillment_tx(AUTHORITY, &fulfillment, OUTSIDER)),
                    ("Init", initialization_tx()),
                ],
            ),
            "finalized",
        )
        .unwrap();
        assert_eq!(state.seq, 1);
        assert_eq!(state.prev_signature.as_deref(), Some("Marker"));
    }

    #[test]
    fn fulfillment_reference_cannot_reuse_program_or_sysvar_key() {
        for reserved in [
            nonce::SYSTEM_PROGRAM_ID_B58,
            memo::MEMO_PROGRAM_ID_B58,
            nonce::RECENT_BLOCKHASHES_SYSVAR_B58,
        ] {
            let fulfillment = json!({
                "v":1, "dev":"k01", "seq":0, "ts":1_000_000,
                "prev":null, "tag":memo::FULFILLMENT_TAG,
                "ref":reserved, "item":"cold_drink",
                "payment_sig":"1111111111111111111111111111111111111111111111111111111111111111"
            })
            .to_string();
            let err = recover(
                DEVICE,
                AUTHORITY,
                "k01",
                mock(
                    json!([entry("Marker", Some(&fulfillment)), entry("Init", None),]),
                    vec![
                        ("Marker", fulfillment_tx(AUTHORITY, &fulfillment, reserved)),
                        ("Init", initialization_tx()),
                    ],
                ),
                "finalized",
            )
            .unwrap_err();
            assert!(
                matches!(err, ChainError::Gap(ref message) if message.contains("infrastructure")),
                "reserved reference {reserved} became a chain link: {err:?}"
            );
        }
    }

    #[test]
    fn unauthenticated_matching_memo_cannot_poison_sequence() {
        let fake = reading_memo(999, None);
        let state = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Fake", Some(&fake)), entry("Init", None)]),
                vec![
                    ("Fake", attestation_tx(OUTSIDER, &fake)),
                    ("Init", initialization_tx()),
                ],
            ),
            "finalized",
        )
        .unwrap();
        assert_eq!(state.seq, 0);
        assert_eq!(state.prev_signature, None);
    }

    #[test]
    fn full_scan_without_authenticated_head_is_a_gap_not_genesis() {
        let entries = (0..CHAIN_SCAN_LIMIT)
            .map(|i| entry(&format!("S{i}"), None))
            .collect::<Vec<_>>();
        let txs = (0..CHAIN_SCAN_LIMIT)
            .map(|i| (format!("S{i}"), public_junk_tx()))
            .collect::<Vec<_>>();
        let transactions = txs
            .iter()
            .map(|(signature, tx)| (signature.as_str(), tx.clone()))
            .collect();
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(Value::Array(entries), transactions),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn authenticated_sequence_overflow_fails_closed() {
        let memo = reading_memo(u64::MAX, None);
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Max", Some(&memo))]),
                vec![("Max", attestation_tx(AUTHORITY, &memo))],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn rpc_error_surfaces_never_silently_fresh() {
        let m = Mock {
            sigs: Err(RpcError::Transport("node down".into())),
            transactions: HashMap::new(),
        };
        let err = recover(DEVICE, AUTHORITY, "k01", m, "finalized").unwrap_err();
        assert!(matches!(err, ChainError::Rpc(_)), "got {err:?}");
    }

    #[test]
    fn empty_history_is_a_gap_not_genesis() {
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(json!([]), vec![]),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn short_public_only_history_is_a_gap_not_genesis() {
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Junk", None)]),
                vec![("Junk", public_junk_tx())],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn pruned_listed_transaction_is_a_gap() {
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Pruned", None)]),
                vec![("Pruned", Value::Null)],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn transaction_memo_must_equal_signature_list_memo() {
        let listed = reading_memo(7, None);
        let actual = reading_memo(8, None);
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Mismatch", Some(&listed))]),
                vec![("Mismatch", attestation_tx(AUTHORITY, &actual))],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn authority_signed_wrong_device_memo_is_a_gap() {
        let memo = reading_memo_for("other", 7, None);
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("WrongDev", Some(&memo)), entry("Init", None)]),
                vec![
                    ("WrongDev", attestation_tx(AUTHORITY, &memo)),
                    ("Init", initialization_tx()),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn malformed_authority_signed_transaction_blocks_rollback_to_older_head() {
        let old = reading_memo(6, None);
        let malformed = transaction(
            AUTHORITY,
            false,
            vec![memo_ix(r#"{"v":1,"dev":"k01","seq":999,"prev":null}"#)],
        );
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([
                    entry(
                        "Malformed",
                        Some(r#"{"v":1,"dev":"k01","seq":999,"prev":null}"#)
                    ),
                    entry("Older", Some(&old)),
                ]),
                vec![
                    ("Malformed", malformed),
                    ("Older", attestation_tx(AUTHORITY, &old)),
                ],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn wrong_advance_nonce_accounts_are_a_gap() {
        let memo = reading_memo(7, None);
        let wrong = transaction(
            AUTHORITY,
            false,
            vec![
                system_ix(
                    "advanceNonce",
                    json!({
                        "nonceAccount": DEVICE,
                        "recentBlockhashesSysvar": nonce::RECENT_BLOCKHASHES_SYSVAR_B58,
                        "nonceAuthority": OUTSIDER,
                    }),
                ),
                memo_ix(&memo),
            ],
        );
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("WrongAdvance", Some(&memo))]),
                vec![("WrongAdvance", wrong)],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn initialization_requires_exact_nonce_creation_shape() {
        let mut wrong = initialization_tx();
        wrong["transaction"]["message"]["instructions"][0]["parsed"]["info"]["space"] = json!(79);
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(json!([entry("BadInit", None)]), vec![("BadInit", wrong)]),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }

    #[test]
    fn failed_public_transaction_does_not_hide_valid_initialization() {
        let entries = json!([
            { "signature": "Failed", "slot": 101, "err": { "InstructionError": [0, "Custom"] }, "memo": null },
            entry("Init", None),
        ]);
        let state = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(entries, vec![("Init", initialization_tx())]),
            "finalized",
        )
        .unwrap();
        assert_eq!(state.seq, 0);
        assert_eq!(state.prev_signature, None);
    }

    #[test]
    fn inconsistent_success_status_fails_closed() {
        let mut failed_tx = public_junk_tx();
        failed_tx["meta"]["err"] = json!({ "InstructionError": [0, "Custom"] });
        let err = recover(
            DEVICE,
            AUTHORITY,
            "k01",
            mock(
                json!([entry("Inconsistent", None), entry("Init", None)]),
                vec![("Inconsistent", failed_tx), ("Init", initialization_tx())],
            ),
            "finalized",
        )
        .unwrap_err();
        assert!(matches!(err, ChainError::Gap(_)), "got {err:?}");
    }
}
