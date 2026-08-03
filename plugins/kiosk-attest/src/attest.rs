//! Pure attestation core. No wasm dependency — RPC mocked in host tests.
//!
//! Custody: T1. This module holds no key and signs nothing. It builds an
//! UNSIGNED transaction from exactly two instructions — System `AdvanceNonceAccount`
//! (durable nonce) and SPL Memo (the hash-chained attestation record) — and hands
//! back its base64 message for an external operator signer. A transfer is not
//! constructed anywhere, so even a fully compromised model cannot make this
//! plugin emit a spend (proven by `tx_contains_only_memo_and_system_programs`).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use kiosk_core::msg::Message;
use kiosk_core::rpc::{RpcClient, RpcError, RpcTransport};
use kiosk_core::{b58, chain, memo, nonce, shape};

pub const MEMO_VERSION: u32 = 1;
pub const DEFAULT_FINALITY: &str = "finalized";
const MAX_DEVICE_ID_BYTES: usize = 64;
const MAX_LABEL_BYTES: usize = 64;
const MAX_PAYMENT_SIG_BYTES: usize = 128;
const MAX_ALLOWED_METRICS_BYTES: usize = 4 * 1024;
const MAX_ALLOWED_METRICS_COUNT: usize = 64;
/// Solana's serialized transaction packet limit. One required signature adds a
/// one-byte compact length plus 64 signature bytes to the serialized message.
const MAX_TRANSACTION_BYTES: usize = 1232;
const ONE_SIGNATURE_SECTION_BYTES: usize = 65;

#[derive(Debug)]
pub struct AttestConfig {
    pub rpc_url: String,
    pub device_id: String,
    pub nonce_account: String,
    pub nonce_authority: String,
    /// metric name -> inclusive [min, max] bounds.
    pub allowed_metrics: HashMap<String, (f64, f64)>,
    pub custody_mode: String,
}

impl AttestConfig {
    pub fn from_section(section: &HashMap<String, String>) -> Result<Self, AttestError> {
        let get_req = |k: &str| {
            section
                .get(k)
                .filter(|v| !v.is_empty())
                .cloned()
                .ok_or_else(|| AttestError::Config(format!("{k} is required")))
        };
        let rpc_url = get_req("rpc_url")?;
        let device_id = get_req("device_id")?;
        validate_text("device_id", &device_id, MAX_DEVICE_ID_BYTES).map_err(AttestError::Config)?;
        let nonce_account = get_req("nonce_account")?;
        let nonce_authority = get_req("nonce_authority")?;
        if b58::decode_pubkey(&nonce_account).is_none() {
            return Err(AttestError::Config(
                "nonce_account is not a valid pubkey".into(),
            ));
        }
        if b58::decode_pubkey(&nonce_authority).is_none() {
            return Err(AttestError::Config(
                "nonce_authority is not a valid pubkey".into(),
            ));
        }
        let mut allowed_metrics = HashMap::new();
        if let Some(raw) = section.get("allowed_metrics") {
            if raw.len() > MAX_ALLOWED_METRICS_BYTES {
                return Err(AttestError::Config(format!(
                    "allowed_metrics exceeds {MAX_ALLOWED_METRICS_BYTES} UTF-8 bytes"
                )));
            }
            let raw = raw.trim();
            let entries: Vec<&str> = if raw.is_empty() {
                Vec::new()
            } else {
                raw.split(',').collect()
            };
            if entries.len() > MAX_ALLOWED_METRICS_COUNT {
                return Err(AttestError::Config(format!(
                    "allowed_metrics exceeds {MAX_ALLOWED_METRICS_COUNT} entries"
                )));
            }
            for raw_entry in entries {
                let entry = raw_entry.trim();
                if entry.is_empty() {
                    return Err(AttestError::Config(
                        "allowed_metrics contains an empty entry".into(),
                    ));
                }
                let parts: Vec<&str> = entry.split(':').map(str::trim).collect();
                if parts.len() != 3 {
                    return Err(AttestError::Config(format!(
                        "bad allowed_metrics entry `{entry}` (want name:min:max)"
                    )));
                }
                let min = parts[1]
                    .parse::<f64>()
                    .map_err(|_| AttestError::Config(format!("bad min in `{entry}`")))?;
                let max = parts[2]
                    .parse::<f64>()
                    .map_err(|_| AttestError::Config(format!("bad max in `{entry}`")))?;
                validate_metric_name(parts[0]).map_err(AttestError::Config)?;
                if !min.is_finite() || !max.is_finite() {
                    return Err(AttestError::Config(format!(
                        "bounds must be finite in `{entry}`"
                    )));
                }
                if min > max {
                    return Err(AttestError::Config(format!("min > max in `{entry}`")));
                }
                if allowed_metrics
                    .insert(parts[0].to_string(), (min, max))
                    .is_some()
                {
                    return Err(AttestError::Config(format!(
                        "duplicate allowed_metrics entry `{}`",
                        parts[0]
                    )));
                }
            }
        }
        let custody_mode = section
            .get("custody_mode")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| "t1".to_string());
        if custody_mode != "t1" {
            return Err(AttestError::Config(format!(
                "custody_mode `{custody_mode}` is unsupported; kiosk-attest is T1 and only builds unsigned transactions"
            )));
        }
        Ok(Self {
            rpc_url,
            device_id,
            nonce_account,
            nonce_authority,
            allowed_metrics,
            custody_mode,
        })
    }
}

/// Model-facing arguments. `deny_unknown_fields` makes a smuggled `recipient`,
/// `nonce_authority`, … a hard deserialization error.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct AttestArgs {
    /// "reading" (default), "event", or "fulfillment".
    pub kind: Option<String>,
    pub metric: Option<String>,
    pub value: Option<f64>,
    pub event: Option<String>,
    pub payment_sig: Option<String>,
    pub item: Option<String>,
    /// Fulfillment only: the Solana Pay reference of the charge that was just
    /// delivered. It becomes a read-only account key of the marker, which is
    /// what makes the marker discoverable by `kiosk_watch`'s scan of that
    /// reference, providing the watcher's bounded on-chain replay marker.
    pub reference: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum AttestError {
    Config(String),
    Args(String),
    /// A caller value failed the operator allowlist/bounds — refuse to attest a lie.
    Rejected(String),
    Rpc(String),
    Decode(String),
    Clock(String),
}

impl core::fmt::Display for AttestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AttestError::Config(m) => write!(f, "config error: {m}"),
            AttestError::Args(m) => write!(f, "invalid request: {m}"),
            AttestError::Rejected(m) => write!(f, "reading rejected: {m}"),
            AttestError::Rpc(m) => write!(f, "rpc error: {m}"),
            AttestError::Decode(m) => write!(f, "malformed rpc response: {m}"),
            AttestError::Clock(m) => write!(f, "clock error: {m}"),
        }
    }
}

#[derive(Debug)]
pub struct AttestOutput {
    /// Base64 of the UNSIGNED serialized message (0 signatures attached).
    pub tx_base64: String,
    pub seq: u64,
    pub message: Message,
    pub summary: String,
}

impl AttestOutput {
    /// The unique program ids invoked by the built transaction. The safety
    /// invariant: this is exactly {System, Memo} — no transfer/token program.
    pub fn program_ids(&self) -> Vec<[u8; 32]> {
        let mut ids: Vec<[u8; 32]> = self
            .message
            .instructions
            .iter()
            .map(|ci| self.message.account_keys[ci.program_id_index as usize])
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Structured output for callers and signer sidecars. The opaque base64 is
    /// never passed through a prose/token clamp, which could silently corrupt
    /// a transaction while reporting success.
    pub fn machine_output(&self) -> String {
        json!({
            "v": MEMO_VERSION,
            "success": true,
            "status": "signature_required",
            "seq": self.seq,
            "summary": self.summary,
            "unsigned_message_base64": self.tx_base64,
        })
        .to_string()
    }
}

/// Build the unsigned attestation transaction. Validation happens BEFORE any
/// RPC; any RPC/decode failure returns `Err` (never a "successful" attestation).
pub fn execute_attest<T: RpcTransport>(
    args: &AttestArgs,
    cfg: &AttestConfig,
    transport: T,
    now: u64,
) -> Result<AttestOutput, AttestError> {
    // The record time is supplied by the trusted host wrapper, never by the
    // model-facing request. This prevents callers from backdating or
    // postdating otherwise valid readings and fulfillment receipts.
    let ts = now;
    let kind = args.kind.as_deref().unwrap_or("reading");

    // Set by the fulfillment kind: the charge reference, attached to the
    // transaction so `kiosk_watch` can find this marker when it scans that
    // reference. Nothing else adds account keys.
    let mut reference_key: Option<[u8; 32]> = None;

    // 1. Validate the payload against the operator allowlist FIRST (fail closed).
    let (body, detail) = match kind {
        "reading" => {
            let metric = args
                .metric
                .as_deref()
                .ok_or_else(|| AttestError::Args("metric is required for a reading".into()))?;
            let value = args
                .value
                .ok_or_else(|| AttestError::Args("value is required for a reading".into()))?;
            validate_metric_name(metric).map_err(AttestError::Args)?;
            let (min, max) = cfg.allowed_metrics.get(metric).ok_or_else(|| {
                AttestError::Rejected(format!("metric `{metric}` not in allowlist"))
            })?;
            if !value.is_finite() {
                return Err(AttestError::Rejected("value must be finite".into()));
            }
            if value < *min || value > *max {
                return Err(AttestError::Rejected(format!(
                    "value {value} outside [{min}, {max}]"
                )));
            }
            (
                json!({ "metric": metric, "val": value }),
                format!("metric={metric} val={value}"),
            )
        }
        "event" => {
            let event = args
                .event
                .as_deref()
                .ok_or_else(|| AttestError::Args("event is required for an event".into()))?;
            validate_text("event", event, MAX_LABEL_BYTES).map_err(AttestError::Args)?;
            let mut b = json!({ "event": event });
            if let Some(item) = &args.item {
                validate_text("item", item, MAX_LABEL_BYTES).map_err(AttestError::Args)?;
                b["item"] = json!(item);
            }
            if let Some(sig) = &args.payment_sig {
                validate_text("payment_sig", sig, MAX_PAYMENT_SIG_BYTES)
                    .map_err(AttestError::Args)?;
                b["payment_sig"] = json!(sig);
            }
            (b, format!("event={event}"))
        }
        // The delivery receipt used as a bounded on-chain replay marker. It records what
        // was handed over, and — because the charge reference rides along as a
        // read-only key — it is discoverable by the same scan kiosk-watch
        // already performs. Still memo + advance-nonce only: this proves a
        // delivery happened, it cannot move anything.
        "fulfillment" => {
            let reference = args.reference.as_deref().ok_or_else(|| {
                AttestError::Args("reference is required for a fulfillment marker".into())
            })?;
            reference_key = Some(b58::decode_pubkey(reference).ok_or_else(|| {
                AttestError::Args("reference is not a valid 32-byte base58 pubkey".into())
            })?);
            let item = args.item.as_deref().ok_or_else(|| {
                AttestError::Args("item is required for a fulfillment marker".into())
            })?;
            validate_text("item", item, MAX_LABEL_BYTES).map_err(AttestError::Args)?;
            let payment_sig = args.payment_sig.as_deref().ok_or_else(|| {
                AttestError::Args("payment_sig is required for a fulfillment marker".into())
            })?;
            validate_text("payment_sig", payment_sig, MAX_PAYMENT_SIG_BYTES)
                .map_err(AttestError::Args)?;
            if b58::decode(payment_sig).is_none_or(|decoded| decoded.len() != 64) {
                return Err(AttestError::Args(
                    "payment_sig is not a 64-byte base58 Solana signature".into(),
                ));
            }
            let b = json!({
                "tag": memo::FULFILLMENT_TAG,
                "ref": reference,
                "item": item,
                "payment_sig": payment_sig,
            });
            (b, format!("ref={reference}"))
        }
        other => return Err(AttestError::Args(format!("unknown kind `{other}`"))),
    };

    let nonce_account = b58::decode_pubkey(&cfg.nonce_account)
        .ok_or_else(|| AttestError::Config("nonce_account invalid".into()))?;
    let nonce_authority = b58::decode_pubkey(&cfg.nonce_authority)
        .ok_or_else(|| AttestError::Config("nonce_authority invalid".into()))?;
    if let Some(reference) = reference_key {
        if chain::attestation_reference_collides(&reference, &nonce_account, &nonce_authority) {
            return Err(AttestError::Args(
                "reference must be an independent charge key and cannot equal a nonce, authority, sysvar, or program account"
                    .into(),
            ));
        }
    }

    // 2. Snapshot the durable nonce BEFORE recovering chain history. If another
    // attestation lands concurrently after this read, it advances the nonce and
    // makes this artifact unsubmitable. Reading in the opposite order allowed a
    // stale sequence/head to be combined with the newly advanced nonce.
    let client = RpcClient::new(&transport);
    let info = client
        .call(
            "getAccountInfo",
            json!([cfg.nonce_account, { "encoding": "base64", "commitment": DEFAULT_FINALITY }]),
        )
        .map_err(map_rpc)?;
    let min_context_slot = info
        .pointer("/context/slot")
        .and_then(Value::as_u64)
        .ok_or_else(|| AttestError::Decode("nonce account response has no context.slot".into()))?;
    let account = info
        .get("value")
        .filter(|v| !v.is_null())
        .ok_or_else(|| AttestError::Rpc("nonce account not found".into()))?;
    if account.get("owner").and_then(Value::as_str) != Some(nonce::SYSTEM_PROGRAM_ID_B58) {
        return Err(AttestError::Decode(
            "nonce account is not owned by the System Program".into(),
        ));
    }
    if account.get("executable").and_then(Value::as_bool) != Some(false) {
        return Err(AttestError::Decode(
            "nonce account must be non-executable".into(),
        ));
    }
    let data = account
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AttestError::Decode("nonce account data is not base64 tuple".into()))?;
    if data.get(1).and_then(Value::as_str) != Some("base64") {
        return Err(AttestError::Decode(
            "nonce account data encoding is not base64".into(),
        ));
    }
    let data_b64 = data
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| AttestError::Decode("nonce account has no base64 data".into()))?;
    let na = nonce::parse_nonce_account(data_b64)
        .ok_or_else(|| AttestError::Decode("account is not a valid durable nonce".into()))?;
    if na.authority != nonce_authority {
        return Err(AttestError::Config(
            "configured nonce_authority does not own the nonce account".into(),
        ));
    }

    // 3. Recover the newest finalized, authority-signed chain head. Public
    // traffic and the nonce-creation transaction do not become chain state.
    let state = chain::recover_at_min_context_slot(
        &cfg.nonce_account,
        &cfg.nonce_authority,
        &cfg.device_id,
        &transport,
        DEFAULT_FINALITY,
        Some(min_context_slot),
    )
    .map_err(|e| match e {
        chain::ChainError::Rpc(m) => AttestError::Rpc(m),
        chain::ChainError::Decode(m) => AttestError::Decode(m),
        chain::ChainError::Gap(m) => AttestError::Rejected(format!("chain gap: {m}")),
    })?;

    // 4. Assemble the hash-chained memo record.
    let mut memo_val =
        json!({ "v": MEMO_VERSION, "dev": cfg.device_id, "seq": state.seq, "ts": ts });
    if let (Value::Object(m), Value::Object(b)) = (&mut memo_val, &body) {
        for (k, v) in b {
            m.insert(k.clone(), v.clone());
        }
    }
    memo_val["prev"] = match &state.prev_signature {
        Some(s) => json!(s),
        None => Value::Null,
    };
    let memo_json = memo_val.to_string();

    // 5. Compile [advance-nonce, memo] into an UNSIGNED message on the durable nonce.
    // The charge reference hangs off the advance-nonce instruction, not the
    // memo: SPL Memo v2 requires every account passed to it to be a signer, and
    // the kiosk cannot sign for a reference keypair it does not hold. The System
    // program reads only accounts 0..=2 of AdvanceNonceAccount and ignores the
    // rest, so an extra read-only key is inert on-chain while still landing the
    // transaction in getSignaturesForAddress(reference) — the same mechanism
    // Solana Pay uses to make a payment findable by its reference.
    let mut advance = nonce::build_advance_nonce_ix(nonce_account, nonce_authority);
    if let Some(key) = reference_key {
        advance.accounts.push(memo::AccountMeta {
            pubkey: key,
            is_signer: false,
            is_writable: false,
        });
    }
    let memo_ix = memo::build_memo_ix(&memo_json);
    let message = Message::compile(&[advance, memo_ix], nonce_authority, na.blockhash);
    let message_bytes = message.serialize();
    if message_bytes
        .len()
        .saturating_add(ONE_SIGNATURE_SECTION_BYTES)
        > MAX_TRANSACTION_BYTES
    {
        return Err(AttestError::Rejected(format!(
            "attestation transaction would exceed {MAX_TRANSACTION_BYTES} bytes"
        )));
    }
    let tx_base64 = message.to_base64();

    // 6. Summary: status only, no secrets, no rpc_url.
    let summary = shape::clamp(
        &format!(
            "BUILT {kind} seq={} {detail} ts={ts} — signature required; unsigned durable-nonce message is {} bytes.",
            state.seq,
            message_bytes.len()
        ),
        shape::DEFAULT_BUDGET_TOKENS,
    );

    Ok(AttestOutput {
        tx_base64,
        seq: state.seq,
        message,
        summary,
    })
}

fn validate_text(name: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > max_bytes {
        return Err(format!("{name} exceeds {max_bytes} UTF-8 bytes"));
    }
    Ok(())
}

fn validate_metric_name(value: &str) -> Result<(), String> {
    validate_text("metric name", value, MAX_LABEL_BYTES)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err("metric name may contain only ASCII letters, digits, `_`, `-`, and `.`".into());
    }
    Ok(())
}

/// Convert a host clock reading to Unix seconds without silently substituting
/// epoch zero when the platform clock is invalid.
pub fn unix_timestamp(time: SystemTime) -> Result<u64, AttestError> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| AttestError::Clock("system clock is before the Unix epoch".into()))
}

fn map_rpc(e: RpcError) -> AttestError {
    match e {
        RpcError::Transport(m) => AttestError::Rpc(m),
        RpcError::Rpc { code, message } => AttestError::Rpc(format!("{code}: {message}")),
        RpcError::Decode(m) => AttestError::Decode(m),
    }
}
