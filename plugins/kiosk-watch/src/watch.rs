//! Pure verification core. No wasm dependency — compiles and tests on the host
//! with RPC mocked through [`kiosk_core::rpc::RpcTransport`].
//!
//! Custody: T0. This module holds no key and signs nothing. It performs
//! read-only Solana JSON-RPC calls to answer one question the actuation SOP
//! gates on: *did the expected payment land on-chain?* — and a companion
//! question: *is the device's attestation heartbeat fresh?*
//!
//! The single load-bearing invariant: **an RPC or decode failure is NEVER
//! reported as Paid.** Every network/shape error returns `Err`, so the shim
//! maps it to `success:false` and the relay stays shut. The only path to a
//! `Paid` verdict is a fully parsed transaction that credits the exact
//! configured item price in the operator's `usdc_mint` to the operator's
//! `merchant_address` and references this charge.

use std::collections::{HashMap, HashSet};

use kiosk_core::b58;
use kiosk_core::pay::{amount_to_base_units, canonicalize_amount};
use kiosk_core::rpc::{RpcClient, RpcError, RpcTransport};
use kiosk_core::{chain, memo::MEMO_PROGRAM_ID_B58, shape};
use serde_json::{json, Value};

/// Mainnet USDC mint — the shipped default. Operators override in config
/// (e.g. devnet USDC); never the model.
pub const DEFAULT_USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const USDC_DECIMALS: u8 = 6;
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
const TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM_ID: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const COMPUTE_BUDGET_PROGRAM_ID: &str = "ComputeBudget111111111111111111111111111111";
/// How many recent signatures to pull on the reference / device address.
const SIG_LIMIT: u64 = 10;
const PRICE_LIST_MAX_BYTES: usize = 4_096;
pub const DEFAULT_PAYMENT_WINDOW_S: u64 = 900;
pub const DEFAULT_HEARTBEAT_MAX_SILENCE_S: u64 = 1_800;
const MAX_POLICY_WINDOW_S: u64 = 86_400;
/// Nodes and kiosk clocks can differ slightly, but a transaction timestamp far
/// in the future must never become a fresh payment/heartbeat indefinitely.
const MAX_FUTURE_BLOCKTIME_SKEW_S: u64 = 30;
/// Memo tag marking a delivery as fulfilled — written by `kiosk-attest`'s
/// fulfillment kind, recognised here. Defined once in `kiosk-core` so the
/// writer and the reader of this wire contract cannot drift apart.
pub use kiosk_core::memo::{FULFILLMENT_TAG, PAYMENT_TAG};
/// The only commitment a payment verdict may actuate on. `processed` can still
/// be rolled back and `confirmed` is merely reorg-*unlikely*; dispensing a
/// physical item is not reversible, so the verdict that opens a relay demands
/// economic irreversibility. Heartbeat mode is unaffected — it does not actuate.
pub const ACTUATION_FINALITY: &str = "finalized";
/// An SPL mint stores `decimals` as a u8 but no real mint exceeds this; a
/// larger value means the response is not to be trusted for scaling money.
const MAX_MINT_DECIMALS: u8 = 18;

/// Operator configuration, injected by the host as `__config`. Fail closed:
/// without an RPC endpoint and a valid merchant address the plugin refuses.
#[derive(Debug)]
pub struct WatchConfig {
    pub rpc_url: String,
    pub merchant_address: String,
    pub usdc_mint: String,
    /// Decimal precision of the configured mint. This is operator-owned and
    /// must match kiosk-charge; transaction balances and transfer instructions
    /// are checked against it before a payment can actuate.
    pub token_decimals: u8,
    /// item id -> decimal amount string, parsed from `price_list`
    /// (`"cold_drink:1.5, snack:0.75"`) — the SAME key and format
    /// `kiosk-charge` parses, so the price that gates the relay is the price
    /// the customer was quoted. This is the only source of the expected amount;
    /// the model supplies an item id and nothing else.
    pub price_list: HashMap<String, String>,
    /// The operator's device authority: the ONLY signer whose fulfillment
    /// marker counts. **Must equal `kiosk-attest`'s `nonce_authority`** — that
    /// is the fee payer, and only required signer, of every marker kiosk-attest
    /// builds. Set them to different pubkeys and no marker will ever
    /// authenticate, so the bounded on-chain replay barrier silently stops
    /// working; `scripts/check-config.sh` checks the two agree.
    pub device_authority: Option<String>,
    /// Public nonce/device address whose authenticated attestations form the
    /// heartbeat. Operator-owned: a model cannot redirect liveness checks.
    pub device_address: Option<String>,
    /// Expected `dev` field in the signed attestation memo.
    pub device_id: Option<String>,
    /// Operator-owned quote lifetime. A paid result carries this policy and
    /// the verified payment block time; the trusted claim compares both with
    /// the persisted charge creation time before actuation.
    pub payment_window_s: u64,
    /// Operator-owned heartbeat alert threshold.
    pub heartbeat_max_silence_s: u64,
    /// Solana commitment gating the answer: processed | confirmed | finalized.
    pub finality: String,
}

impl WatchConfig {
    pub fn from_section(section: &HashMap<String, String>) -> Result<Self, WatchError> {
        let rpc_url = section
            .get("rpc_url")
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or_else(|| WatchError::Config("rpc_url is required".into()))?;
        let merchant_address = section
            .get("merchant_address")
            .cloned()
            .ok_or_else(|| WatchError::Config("merchant_address is required".into()))?;
        if b58::decode_pubkey(&merchant_address).is_none() {
            return Err(WatchError::Config(
                "merchant_address is not a valid 32-byte base58 pubkey".into(),
            ));
        }
        let usdc_mint = section
            .get("usdc_mint")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| DEFAULT_USDC_MINT.to_string());
        if b58::decode_pubkey(&usdc_mint).is_none() {
            return Err(WatchError::Config("usdc_mint is not a valid pubkey".into()));
        }
        let token_decimals = section
            .get("token_decimals")
            .filter(|value| !value.is_empty())
            .map(|value| {
                value.parse::<u8>().map_err(|_| {
                    WatchError::Config("token_decimals must be an integer from 0 to 18".into())
                })
            })
            .transpose()?
            .unwrap_or(USDC_DECIMALS);
        if token_decimals > MAX_MINT_DECIMALS {
            return Err(WatchError::Config(format!(
                "token_decimals must be between 0 and {MAX_MINT_DECIMALS}"
            )));
        }
        // Same key, same format as ChargeConfig. Each price is parsed here so an
        // operator typo fails at config load rather than at actuation time.
        let mut price_list = HashMap::new();
        if let Some(raw) = section.get("price_list") {
            if raw.len() > PRICE_LIST_MAX_BYTES {
                return Err(WatchError::Config(format!(
                    "price_list exceeds {PRICE_LIST_MAX_BYTES} bytes"
                )));
            }
            for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                let (item, amount) = entry
                    .split_once(':')
                    .ok_or_else(|| WatchError::Config(format!("bad price entry `{entry}`")))?;
                let (item, amount) = (item.trim(), amount.trim());
                if item.is_empty()
                    || item.len() > 64
                    || !item
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(WatchError::Config(
                        "price-list item ids must use 1 to 64 ASCII letters, digits, `_`, or `-`"
                            .into(),
                    ));
                }
                let amount = canonicalize_amount(amount, token_decimals).map_err(|error| {
                    WatchError::Config(format!("invalid price for item `{item}`: {error}"))
                })?;
                if price_list.insert(item.to_string(), amount).is_some() {
                    return Err(WatchError::Config(format!(
                        "duplicate price-list item `{item}`"
                    )));
                }
            }
        }
        let device_authority = section
            .get("device_authority")
            .filter(|v| !v.is_empty())
            .cloned();
        if let Some(da) = &device_authority {
            if b58::decode_pubkey(da).is_none() {
                return Err(WatchError::Config(
                    "device_authority is not a valid 32-byte base58 pubkey".into(),
                ));
            }
        }
        let device_address = section
            .get("device_address")
            .filter(|v| !v.is_empty())
            .cloned();
        if let Some(address) = &device_address {
            if b58::decode_pubkey(address).is_none() {
                return Err(WatchError::Config(
                    "device_address is not a valid 32-byte base58 pubkey".into(),
                ));
            }
        }
        let device_id = section.get("device_id").filter(|v| !v.is_empty()).cloned();
        if device_id.as_ref().is_some_and(|id| id.len() > 64) {
            return Err(WatchError::Config(
                "device_id exceeds 64 UTF-8 bytes".into(),
            ));
        }
        let payment_window_s =
            policy_seconds(section, "payment_window_s", DEFAULT_PAYMENT_WINDOW_S)?;
        let heartbeat_max_silence_s = policy_seconds(
            section,
            "heartbeat_max_silence_s",
            DEFAULT_HEARTBEAT_MAX_SILENCE_S,
        )?;
        // Defaults to the safe end: an operator who never thinks about
        // commitment gets irreversibility, not a silent downgrade. Weaker
        // settings remain legal for heartbeat mode, which does not actuate.
        let finality = section
            .get("finality")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| ACTUATION_FINALITY.to_string());
        if !matches!(finality.as_str(), "processed" | "confirmed" | "finalized") {
            return Err(WatchError::Config(
                "finality must be processed, confirmed, or finalized".into(),
            ));
        }
        Ok(Self {
            rpc_url,
            merchant_address,
            usdc_mint,
            token_decimals,
            price_list,
            device_authority,
            device_address,
            device_id,
            payment_window_s,
            heartbeat_max_silence_s,
            finality,
        })
    }
}

fn policy_seconds(
    section: &HashMap<String, String>,
    key: &str,
    default: u64,
) -> Result<u64, WatchError> {
    let value = match section.get(key).filter(|value| !value.is_empty()) {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| WatchError::Config(format!("{key} must be an integer")))?,
        None => default,
    };
    if !(1..=MAX_POLICY_WINDOW_S).contains(&value) {
        return Err(WatchError::Config(format!(
            "{key} must be between 1 and {MAX_POLICY_WINDOW_S} seconds"
        )));
    }
    Ok(value)
}

/// Model-facing arguments. `deny_unknown_fields` makes smuggled keys
/// (`rpc_url`, `merchant_address`, …) a hard deserialization error. Every
/// field is optional at the serde layer; presence is enforced per mode in the
/// core, so one struct serves both payment and heartbeat calls.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct WatchArgs {
    /// `"heartbeat"` selects heartbeat mode; absent/`"payment"` = payment mode.
    pub mode: Option<String>,
    /// Payment mode: the Solana Pay reference pubkey from the charge.
    pub reference: Option<String>,
    /// Payment mode: the item id this charge was created for. The expected
    /// amount is looked up from the operator's `price_list` — there is
    /// deliberately NO amount field here, so the number the relay gates on is
    /// unreachable from the prompt (see docs-local/DECISIONS.md, 2026-08-02).
    pub item_id: Option<String>,
}

/// Payment verification outcome. `Paid` is necessary but not sufficient for
/// actuation: a trusted driver must also atomically claim the persisted order.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Paid {
        payer: String,
        signature: String,
        slot: u64,
        payment_block_time_s: u64,
        reference: String,
        item_id: String,
        amount: String,
        recipient: String,
        mint: String,
        token_decimals: u8,
        payment_window_s: u64,
    },
    /// No matching signature yet — the SOP should keep polling.
    Pending,
    /// A transaction was found but does not match the expected payment.
    Mismatch { reason: String },
    /// This charge already has an authenticated fulfillment marker on-chain:
    /// it was delivered once already. The payment may well be valid — that is
    /// exactly why this is not `Paid`. Single-use is enforced here, not by
    /// remembering anything (the component is stateless by construction).
    AlreadyFulfilled,
}

/// Heartbeat outcome for the device's attestation address.
#[derive(Debug, PartialEq)]
pub enum Heartbeat {
    Live { signature: String, age_s: u64 },
    Stale { signature: String, age_s: u64 },
    Missing,
}

/// Failure taxonomy. `Rpc` and `Decode` exist so a network or shape failure is
/// structurally distinct from a verdict — and can NEVER be a `Paid`.
#[derive(Debug, PartialEq)]
pub enum WatchError {
    Config(String),
    Args(String),
    Rpc(String),
    Decode(String),
}

impl core::fmt::Display for WatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WatchError::Config(m) => write!(f, "config error: {m}"),
            WatchError::Args(m) => write!(f, "invalid request: {m}"),
            WatchError::Rpc(m) => write!(f, "rpc error: {m}"),
            WatchError::Decode(m) => write!(f, "malformed rpc response: {m}"),
        }
    }
}

impl From<RpcError> for WatchError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Transport(m) => WatchError::Rpc(m),
            RpcError::Rpc { code, message } => WatchError::Rpc(format!("{code}: {message}")),
            RpcError::Decode(m) => WatchError::Decode(m),
        }
    }
}

impl Verdict {
    /// Human/LLM-facing summary, token-budgeted (trap #3).
    pub fn summary(&self) -> String {
        let s = match self {
            Verdict::Paid {
                payer,
                signature,
                slot,
                reference,
                item_id,
                amount,
                recipient,
                mint,
                ..
            } => format!(
                "PAID. Payment verified on-chain at slot {slot}, signature {signature}, transfer authority {payer}, reference {reference}, item {item_id}, amount {amount}, recipient {recipient}, mint {mint}. A trusted driver must match the persisted quote and atomically claim it before actuation."
            ),
            Verdict::Pending => {
                "PENDING. No matching payment on-chain yet. Do not deliver; check again shortly.".into()
            }
            Verdict::Mismatch { reason } => {
                format!("MISMATCH. A transaction was found but does not match the charge: {reason}. Do not deliver.")
            }
            Verdict::AlreadyFulfilled => {
                "ALREADY FULFILLED. This charge was already delivered; not re-firing.".into()
            }
        };
        shape::clamp(&s, shape::DEFAULT_BUDGET_TOKENS)
    }

    /// True only for a verified payment. This is not by itself an exactly-once
    /// actuator authorization; the trusted driver must also atomically claim
    /// the persisted order.
    pub fn is_paid(&self) -> bool {
        matches!(self, Verdict::Paid { .. })
    }

    /// Canonical machine-readable output consumed by deterministic SOP routing.
    /// `success` intentionally duplicates the WIT `ToolResult.success` bit:
    /// ZeroClaw v0 routing sees the output JSON, not that outer host field.
    pub fn machine_output(&self) -> String {
        let mut output = match self {
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
            } => json!({
                "v": 1,
                "success": true,
                "status": "paid",
                "payer": shape::clamp(payer, 128),
                "signature": shape::clamp(signature, 128),
                "slot": slot,
                "payment_block_time_s": payment_block_time_s,
                "reference": reference,
                "item_id": item_id,
                "amount": amount,
                "recipient": recipient,
                "mint": mint,
                "token_decimals": token_decimals,
                "payment_window_s": payment_window_s,
                "payment_verified": true,
                "actuation_authorized": false,
                "requires_atomic_claim": true,
            }),
            Verdict::Pending => json!({ "v": 1, "success": false, "status": "pending" }),
            Verdict::Mismatch { reason } => json!({
                "v": 1,
                "success": false,
                "status": "mismatch",
                "reason": shape::clamp(reason, 128),
            }),
            Verdict::AlreadyFulfilled => {
                json!({ "v": 1, "success": false, "status": "already_fulfilled" })
            }
        };
        output["message"] = Value::String(self.summary());
        output.to_string()
    }
}

impl Heartbeat {
    pub fn summary(&self) -> String {
        let s = match self {
            Heartbeat::Live { age_s, signature } => {
                format!("LIVE. Newest device attestation is {age_s}s old (signature {signature}).")
            }
            Heartbeat::Stale { age_s, signature } => format!(
                "STALE. Newest device attestation is {age_s}s old, past the silence threshold (signature {signature}). Alert the operator."
            ),
            Heartbeat::Missing => {
                "MISSING. No attestations found for the device address. Alert the operator.".into()
            }
        };
        shape::clamp(&s, shape::DEFAULT_BUDGET_TOKENS)
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Heartbeat::Live { .. })
    }

    /// Canonical machine-readable heartbeat output for SOP routing.
    pub fn machine_output(&self) -> String {
        let mut output = match self {
            Heartbeat::Live { signature, age_s } => json!({
                "v": 1,
                "success": true,
                "status": "live",
                "signature": shape::clamp(signature, 128),
                "age_s": age_s,
            }),
            Heartbeat::Stale { signature, age_s } => json!({
                "v": 1,
                "success": false,
                "status": "stale",
                "signature": shape::clamp(signature, 128),
                "age_s": age_s,
            }),
            Heartbeat::Missing => json!({ "v": 1, "success": false, "status": "missing" }),
        };
        output["message"] = Value::String(self.summary());
        output.to_string()
    }
}

/// Verify the expected payment on-chain. Returns `Err` (never `Paid`) on any
/// RPC or decode failure. `now` is unix seconds, supplied by the shim/test so
/// the core stays deterministic.
pub fn verify_payment<T: RpcTransport>(
    args: &WatchArgs,
    cfg: &WatchConfig,
    transport: T,
    now: u64,
) -> Result<Verdict, WatchError> {
    let reference = args
        .reference
        .as_deref()
        .filter(|r| b58::decode_pubkey(r).is_some())
        .ok_or_else(|| WatchError::Args("reference must be a valid pubkey".into()))?;
    // The amount the relay gates on comes from operator config, keyed by an item
    // the caller may only *choose*, never *write*. A charge with no item id is a
    // free-amount charge: invoicing-only, and never actuation-eligible.
    let item_id = args.item_id.as_deref().ok_or_else(|| {
        WatchError::Args(
            "item_id is required: this verifies item-priced charges only. A free-amount \
             charge (kiosk_charge amount_usdc) is invoicing-only and is never \
             actuation-eligible."
                .into(),
        )
    })?;
    let expected_amount = cfg.price_list.get(item_id).ok_or_else(|| {
        WatchError::Args(format!(
            "unknown item `{item_id}`: not in the operator price list"
        ))
    })?;

    // Quote lifetime is operator-owned. Watch reports the verified payment's
    // block time and the configured window; the trusted claim boundary binds
    // both to the persisted charge creation time. Applying this window to the
    // *observation* time here would strand a customer who paid on time during a
    // host outage.

    // Actuation requires economic irreversibility. Refuse up front rather than
    // verify against a commitment that can still be rolled back.
    if cfg.finality != ACTUATION_FINALITY {
        return Err(WatchError::Config(format!(
            "finality is `{}`, but a payment verdict that can actuate requires `{ACTUATION_FINALITY}`",
            cfg.finality
        )));
    }

    // The bounded on-chain replay barrier depends on authenticating a
    // fulfillment marker. Without an authority there is no way to distinguish
    // the operator's marker from a stranger's, so refuse rather than disable
    // the check quietly.
    let device_authority = cfg.device_authority.as_deref().ok_or_else(|| {
        WatchError::Config(
            "device_authority is required to verify a payment: without it a fulfillment \
             marker cannot be authenticated and the on-chain replay barrier cannot be enforced"
                .into(),
        )
    })?;
    let device_address = cfg.device_address.as_deref().ok_or_else(|| {
        WatchError::Config(
            "device_address is required to verify a payment: fulfillment markers must advance the configured durable nonce"
                .into(),
        )
    })?;
    let device_id = cfg.device_id.as_deref().ok_or_else(|| {
        WatchError::Config("device_id is required to verify a payment fulfillment marker".into())
    })?;

    let client = RpcClient::new(transport);

    // 1. Any signatures referencing this charge?
    let sigs = client.call(
        "getSignaturesForAddress",
        json!([reference, { "commitment": cfg.finality, "limit": SIG_LIMIT }]),
    )?;
    let sig_list = sigs.as_array().ok_or_else(|| {
        WatchError::Decode("getSignaturesForAddress did not return an array".into())
    })?;
    if sig_list.len() > SIG_LIMIT as usize {
        return Err(WatchError::Decode(format!(
            "getSignaturesForAddress exceeded the requested {SIG_LIMIT}-entry limit"
        )));
    }
    if sig_list.is_empty() {
        return Ok(Verdict::Pending);
    }

    // The reference is public — it is printed in the QR the customer scans — so
    // anyone can write a transaction naming it. Split the list rather than
    // trusting its head: a stranger's junk tx landing after the real payment
    // must not hide it, and a stranger's fake marker must not block it. Every
    // non-payment entry is still inspected because an authority-signed device
    // write must fail closed even when its signature-list memo is absent or
    // malformed.
    let mut markers: Vec<(&str, &Value)> = Vec::new();
    let mut payments: Vec<(&str, &Value)> = Vec::new();
    for entry in sig_list {
        let signature = entry
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| WatchError::Decode("signature entry missing `signature`".into()))?;
        if entry_is_payment_claim(entry, reference, item_id) {
            payments.push((signature, entry));
        } else {
            markers.push((signature, entry));
        }
    }

    // 2. Already delivered? Authenticate every non-payment transaction before
    // trusting its signature-list memo or judging its schema. This deliberately
    // includes missing, future, and malformed marker versions: public lookalikes
    // are ignored after signer verification, while an authority-signed device
    // write referencing this charge must be the exact current fulfillment
    // schema or fail closed. Checked before the payment so a verified payment
    // can never bypass an authenticated unknown marker.
    for (signature, entry) in markers {
        let txv = fetch_transaction(&client, signature, cfg)?;
        if authority_signed_device_message(&txv, device_address, device_authority, Some(reference))?
            .is_none()
        {
            continue;
        }
        let expected_memo = entry_memo_payload(entry).ok_or_else(|| {
            WatchError::Decode("authority-signed fulfillment candidate has no memo payload".into())
        })?;
        if marker_is_authentic(
            &txv,
            reference,
            device_address,
            device_authority,
            device_id,
            item_id,
            expected_memo,
        )? {
            let marker = entry_memo_json(entry).ok_or_else(|| {
                WatchError::Decode("authenticated fulfillment marker memo is not JSON".into())
            })?;
            let payment_signature = marker
                .get("payment_sig")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WatchError::Decode("authenticated fulfillment marker has no payment_sig".into())
                })?;
            let payment_tx = fetch_transaction(&client, payment_signature, cfg)?;
            let payment_block_time_s = payment_tx
                .get("blockTime")
                .and_then(Value::as_i64)
                .filter(|value| *value >= 0)
                .map(|value| value as u64)
                .ok_or_else(|| {
                    WatchError::Decode(
                        "authenticated fulfillment marker payment has no usable blockTime".into(),
                    )
                })?;
            if payment_block_time_s > now.saturating_add(MAX_FUTURE_BLOCKTIME_SKEW_S) {
                return Err(WatchError::Decode(
                    "authenticated fulfillment marker payment blockTime is unreasonably far in the future"
                        .into(),
                ));
            }
            match inspect_transaction(
                &payment_tx,
                reference,
                item_id,
                payment_signature,
                cfg,
                expected_amount,
                payment_block_time_s,
            )? {
                Verdict::Paid {
                    slot: payment_slot, ..
                } => {
                    let marker_slot =
                        entry.get("slot").and_then(Value::as_u64).ok_or_else(|| {
                            WatchError::Decode(
                                "authenticated fulfillment marker has no valid slot".into(),
                            )
                        })?;
                    if payment_slot > marker_slot {
                        return Err(WatchError::Decode(
                            "authenticated fulfillment marker predates its named payment".into(),
                        ));
                    }
                    return Ok(Verdict::AlreadyFulfilled);
                }
                Verdict::Mismatch { reason } => {
                    return Err(WatchError::Decode(format!(
                        "authenticated fulfillment marker names an unverified payment: {reason}"
                    )))
                }
                _ => {
                    return Err(WatchError::Decode(
                        "authenticated fulfillment marker names an unverified payment".into(),
                    ))
                }
            }
        }
    }

    // 3. Payment scan, newest first. The first transaction that fully verifies
    // wins; junk in front of it is skipped rather than fatal.
    let mut first_mismatch: Option<String> = None;
    let mut saw_unbounded = false;
    for (signature, entry) in payments {
        // A price alone does not bind this reference to the quoted SKU, and a
        // single transfer can carry several public reference accounts. Require
        // the versioned memo emitted by kiosk-charge: it names exactly this
        // reference and item, so the same signature cannot clear several
        // same-priced orders or be watched as a different equal-priced item.
        if !entry_is_payment_claim(entry, reference, item_id) {
            first_mismatch.get_or_insert_with(|| {
                "payment memo does not bind this reference to the requested item".into()
            });
            continue;
        }
        // Authenticate a possible device write before applying unauthenticated
        // signature-list metadata such as blockTime. Otherwise an
        // authority-signed future schema with a missing/future timestamp could
        // be skipped and an older payment could reopen the charge.
        let txv = fetch_transaction(&client, signature, cfg)?;
        let expected_memo = entry_memo_payload(entry)
            .ok_or_else(|| WatchError::Decode("payment claim has no memo payload".into()))?;
        if authenticated_device_tx(
            &txv,
            device_address,
            device_authority,
            expected_memo,
            Some(reference),
        )? {
            return Err(WatchError::Decode(
                "authenticated device transaction uses payment schema instead of fulfillment schema"
                    .into(),
            ));
        }

        // The block time is part of the verified result consumed by the
        // trusted order claim. Missing or implausibly future time fails closed.
        // Old observation time is allowed here: the claim compares the landed
        // payment time with the immutable quote expiry, so outage recovery does
        // not strand a customer who paid before that expiry.
        let payment_block_time_s = match entry.get("blockTime").and_then(Value::as_i64) {
            Some(bt) if bt >= 0 => {
                let block_time = bt as u64;
                if block_time > now.saturating_add(MAX_FUTURE_BLOCKTIME_SKEW_S) {
                    first_mismatch.get_or_insert_with(|| {
                        "payment blockTime is unreasonably far in the future".into()
                    });
                    continue;
                }
                block_time
            }
            _ => {
                saw_unbounded = true;
                continue;
            }
        };
        match inspect_transaction(
            &txv,
            reference,
            item_id,
            signature,
            cfg,
            expected_amount,
            payment_block_time_s,
        )? {
            paid @ Verdict::Paid { .. } => return Ok(paid),
            Verdict::Mismatch { reason } => first_mismatch.get_or_insert(reason),
            _ => continue,
        };
    }

    // Nothing verified. Report the most informative negative, all of which
    // leave the relay shut.
    if let Some(reason) = first_mismatch {
        return Ok(Verdict::Mismatch { reason });
    }
    if saw_unbounded {
        return Ok(Verdict::Mismatch {
            reason: "a matching signature has no blockTime, so its age cannot be bounded".into(),
        });
    }
    Ok(Verdict::Pending)
}

fn entry_is_payment_claim(entry: &Value, reference: &str, item_id: &str) -> bool {
    let Some(parsed) = entry_memo_json(entry) else {
        return false;
    };
    payment_memo_matches(&parsed, reference, item_id)
}

fn payment_memo_matches(parsed: &Value, reference: &str, item_id: &str) -> bool {
    parsed
        == &json!({
            "v": 1,
            "tag": PAYMENT_TAG,
            "ref": reference,
            "item": item_id,
        })
}

/// Solana's signature history prefixes SPL Memo text as `[byte_count] `.
/// Ignore that display prefix and parse the exact JSON payload.
fn entry_memo_json(entry: &Value) -> Option<Value> {
    serde_json::from_str(entry_memo_payload(entry)?).ok()
}

fn entry_memo_payload(entry: &Value) -> Option<&str> {
    let memo = entry.get("memo")?.as_str()?;
    if let Some(rest) = memo.strip_prefix('[') {
        let end = rest.find("] ")?;
        return Some(&rest[end + 2..]);
    }
    Some(memo)
}

/// One `getTransaction` call at the operator's finality.
fn fetch_transaction<T: RpcTransport>(
    client: &RpcClient<T>,
    signature: &str,
    cfg: &WatchConfig,
) -> Result<Value, WatchError> {
    client
        .call(
            "getTransaction",
            json!([signature, {
                "commitment": cfg.finality,
                "encoding": "jsonParsed",
                "maxSupportedTransactionVersion": 0
            }]),
        )
        .map_err(WatchError::from)
}

/// Is this tagged transaction really the operator's fulfillment marker?
///
/// Authenticate an exact kiosk-attest fulfillment transaction. A successful
/// authority signature by itself is insufficient: the transaction must advance
/// the configured durable nonce, contain exactly the signature-list memo, and
/// attach this charge reference as one read-only non-signer account.
fn marker_is_authentic(
    txv: &Value,
    reference: &str,
    device_address: &str,
    device_authority: &str,
    device_id: &str,
    item_id: &str,
    expected_memo: &str,
) -> Result<bool, WatchError> {
    if !authenticated_device_tx(
        txv,
        device_address,
        device_authority,
        expected_memo,
        Some(reference),
    )? {
        return Ok(false);
    }
    let memo: Value = serde_json::from_str(expected_memo)
        .map_err(|_| WatchError::Decode("fulfillment transaction memo is not valid JSON".into()))?;
    let allowed = [
        "v",
        "dev",
        "seq",
        "ts",
        "tag",
        "ref",
        "prev",
        "item",
        "payment_sig",
    ];
    if memo.as_object().is_none_or(|object| {
        object.len() != allowed.len() || !object.keys().all(|key| allowed.contains(&key.as_str()))
    }) || memo.get("v").and_then(Value::as_u64) != Some(1)
        || memo.get("dev").and_then(Value::as_str) != Some(device_id)
        || memo.get("seq").and_then(Value::as_u64).is_none()
        || memo.get("ts").and_then(Value::as_u64).is_none()
        || memo.get("tag").and_then(Value::as_str) != Some(FULFILLMENT_TAG)
        || memo.get("ref").and_then(Value::as_str) != Some(reference)
        || !matches!(memo.get("prev"), Some(Value::Null | Value::String(_)))
        || memo.get("item").and_then(Value::as_str) != Some(item_id)
        || memo
            .get("payment_sig")
            .and_then(Value::as_str)
            .and_then(b58::decode)
            .is_none_or(|signature| signature.len() != 64)
    {
        return Err(WatchError::Decode(
            "fulfillment marker does not bind the configured device, item, and payment signature"
                .into(),
        ));
    }
    Ok(true)
}

fn authenticated_device_tx(
    txv: &Value,
    device_address: &str,
    device_authority: &str,
    expected_memo: &str,
    reference: Option<&str>,
) -> Result<bool, WatchError> {
    let Some(message) =
        authority_signed_device_message(txv, device_address, device_authority, reference)?
    else {
        return Ok(false);
    };
    let instructions = message
        .get("instructions")
        .and_then(Value::as_array)
        .ok_or_else(|| WatchError::Decode("fulfillment transaction has no instructions".into()))?;
    if instructions.len() != 2
        || !is_advance_nonce_instruction(&instructions[0], device_address, device_authority)
        || !memo_instruction_matches(&instructions[1], expected_memo)
    {
        return Err(WatchError::Decode(
            "authority-signed device transaction is not exact advanceNonce + Memo".into(),
        ));
    }
    Ok(true)
}

/// Return the message only when a successful transaction is signed by the
/// configured authority and binds the configured device/reference accounts.
/// This envelope check intentionally does not depend on signature-list memo
/// metadata, which is absent for some malformed or future device writes.
fn authority_signed_device_message<'a>(
    txv: &'a Value,
    device_address: &str,
    device_authority: &str,
    reference: Option<&str>,
) -> Result<Option<&'a Value>, WatchError> {
    let meta = txv
        .get("meta")
        .ok_or_else(|| WatchError::Decode("fulfillment transaction has no meta".into()))?;
    let err = meta
        .get("err")
        .ok_or_else(|| WatchError::Decode("fulfillment transaction meta has no err".into()))?;
    if !err.is_null() {
        return Ok(None);
    }
    let message = txv
        .get("transaction")
        .and_then(|value| value.get("message"))
        .ok_or_else(|| WatchError::Decode("fulfillment transaction has no message".into()))?;
    let account_keys = message
        .get("accountKeys")
        .and_then(Value::as_array)
        .ok_or_else(|| WatchError::Decode("fulfillment transaction has no accountKeys".into()))?;
    if !signers(account_keys).any(|key| key == device_authority) {
        return Ok(None);
    }
    if !account_keys
        .iter()
        .any(|key| account_key_pubkey(key) == Some(device_address))
    {
        return Err(WatchError::Decode(
            "authority-signed device transaction omits the configured device account".into(),
        ));
    }
    if let Some(reference) = reference {
        let reference_keys: Vec<&Value> = account_keys
            .iter()
            .filter(|key| account_key_pubkey(key) == Some(reference))
            .collect();
        if reference_keys.len() != 1
            || reference_keys[0].get("signer").and_then(Value::as_bool) != Some(false)
            || reference_keys[0].get("writable").and_then(Value::as_bool) != Some(false)
        {
            return Err(WatchError::Decode(
                "authority-signed fulfillment transaction has an invalid reference account".into(),
            ));
        }
    }
    Ok(Some(message))
}

fn is_advance_nonce_instruction(ix: &Value, nonce: &str, authority: &str) -> bool {
    ix.get("programId").and_then(Value::as_str) == Some(SYSTEM_PROGRAM_ID)
        && ix.get("program").and_then(Value::as_str) == Some("system")
        && ix.pointer("/parsed/type").and_then(Value::as_str) == Some("advanceNonce")
        && ix
            .pointer("/parsed/info/nonceAccount")
            .and_then(Value::as_str)
            == Some(nonce)
        && ix
            .pointer("/parsed/info/nonceAuthority")
            .and_then(Value::as_str)
            == Some(authority)
}

fn memo_instruction_matches(ix: &Value, expected: &str) -> bool {
    ix.get("programId").and_then(Value::as_str) == Some(MEMO_PROGRAM_ID_B58)
        && ix.get("program").and_then(Value::as_str) == Some("spl-memo")
        && ix.get("parsed").and_then(Value::as_str) == Some(expected)
}

/// Pubkeys that signed the transaction. `jsonParsed` marks each key with
/// `signer`; a bare-string `accountKeys` array carries no flags, in which case
/// only index 0 (always the fee payer, always a signer) can be relied on.
fn signers(account_keys: &[Value]) -> impl Iterator<Item = &str> {
    let flagged = account_keys
        .iter()
        .any(|k| k.get("signer").and_then(Value::as_bool).is_some());
    account_keys
        .iter()
        .enumerate()
        .filter(move |(i, k)| {
            if flagged {
                k.get("signer").and_then(Value::as_bool).unwrap_or(false)
            } else {
                *i == 0
            }
        })
        .filter_map(|(_, k)| account_key_pubkey(k))
}

/// Turn a getTransaction result into a [`Verdict`]. Missing structural fields
/// are decode errors (fail closed); business mismatches are `Mismatch`.
fn inspect_transaction(
    txv: &Value,
    reference: &str,
    item_id: &str,
    signature: &str,
    cfg: &WatchConfig,
    expected_amount: &str,
    payment_block_time_s: u64,
) -> Result<Verdict, WatchError> {
    let meta = txv
        .get("meta")
        .ok_or_else(|| WatchError::Decode("transaction has no meta".into()))?;

    // On-chain failure: funds did not move.
    if !meta.get("err").map(Value::is_null).unwrap_or(false) {
        return Ok(Verdict::Mismatch {
            reason: "on-chain transaction failed".into(),
        });
    }

    let message = txv
        .get("transaction")
        .and_then(|t| t.get("message"))
        .ok_or_else(|| WatchError::Decode("transaction has no message".into()))?;
    let account_keys = message
        .get("accountKeys")
        .and_then(Value::as_array)
        .ok_or_else(|| WatchError::Decode("transaction has no accountKeys".into()))?;
    let keys: Vec<&str> = account_keys
        .iter()
        .map(|key| {
            account_key_pubkey(key).ok_or_else(|| {
                WatchError::Decode("transaction contains an invalid account key".into())
            })
        })
        .collect::<Result<_, _>>()?;

    // The tx must reference this charge (defense in depth beyond the lookup):
    // getSignaturesForAddress(reference) should only ever return txs touching
    // the reference, but we re-verify rather than trust the node's index.
    let reference_keys: Vec<&Value> = account_keys
        .iter()
        .filter(|key| account_key_pubkey(key) == Some(reference))
        .collect();
    if reference_keys.len() != 1 {
        return Ok(Verdict::Mismatch {
            reason: "transaction must contain this charge reference exactly once".into(),
        });
    }
    if reference_keys[0].get("signer").and_then(Value::as_bool) != Some(false)
        || reference_keys[0].get("writable").and_then(Value::as_bool) != Some(false)
    {
        return Ok(Verdict::Mismatch {
            reason: "charge reference must be a read-only non-signer".into(),
        });
    }

    let slot = txv
        .get("slot")
        .and_then(Value::as_u64)
        .ok_or_else(|| WatchError::Decode("transaction has no u64 slot".into()))?;
    let expected_units =
        amount_to_base_units(expected_amount, cfg.token_decimals).ok_or_else(|| {
            WatchError::Config(format!(
                "price `{expected_amount}` does not fit configured token_decimals {}",
                cfg.token_decimals
            ))
        })?;
    let transfer = match validate_payment_instructions(
        message,
        account_keys,
        reference,
        item_id,
        &cfg.usdc_mint,
        expected_units,
        cfg.token_decimals,
    ) {
        Ok(transfer) => transfer,
        Err(reason) => return Ok(Verdict::Mismatch { reason }),
    };

    // Credit to the operator's merchant in the operator's mint.
    let pre = token_balances(meta, "preTokenBalances", keys.len())?;
    let post = token_balances(meta, "postTokenBalances", keys.len())?;

    let merchant_post: Vec<&TokenBalance> = post
        .iter()
        .filter(|b| b.owner == cfg.merchant_address && b.mint == cfg.usdc_mint)
        .collect();
    if merchant_post.is_empty() {
        let reason = if post.iter().any(|b| b.mint == cfg.usdc_mint) {
            "payment credited a different recipient"
        } else if post.iter().any(|b| b.owner == cfg.merchant_address) {
            "payment used a different mint"
        } else {
            "no USDC credit to the merchant was found"
        };
        return Ok(Verdict::Mismatch {
            reason: reason.into(),
        });
    }
    let Some(destination_balance) = post
        .iter()
        .find(|balance| balance.account_index == transfer.destination_index)
    else {
        return Ok(Verdict::Mismatch {
            reason: "transfer destination has no post-token balance".into(),
        });
    };
    if destination_balance.owner != cfg.merchant_address
        || destination_balance.mint != cfg.usdc_mint
    {
        return Ok(Verdict::Mismatch {
            reason: "transfer instruction does not target a merchant-owned account in the configured mint"
                .into(),
        });
    }
    // Every balance for this owner+mint must agree on scale. Silently mixing
    // decimals would make the aggregate meaningless.
    if pre
        .iter()
        .chain(post.iter())
        .filter(|b| b.owner == cfg.merchant_address && b.mint == cfg.usdc_mint)
        .any(|b| b.decimals != cfg.token_decimals)
    {
        return Err(WatchError::Decode(
            "merchant token balances disagree on mint decimals".into(),
        ));
    }

    // Compute the merchant's NET delta across every token account in this mint.
    // Looking only at the first credited account lets an internal transfer from
    // one merchant-owned account to another masquerade as customer payment.
    // Every post-balance account must have a pre entry so a pre-existing balance
    // can never be mistaken for a fresh credit.
    if merchant_post.iter().any(|post_balance| {
        !pre.iter()
            .any(|pre_balance| pre_balance.account_index == post_balance.account_index)
    }) {
        return Err(WatchError::Decode(
            "transaction has a merchant post-balance without a matching pre-balance; net credit cannot be determined"
                .into(),
        ));
    }
    let sum = |balances: &[TokenBalance]| -> Result<u128, WatchError> {
        balances
            .iter()
            .filter(|b| b.owner == cfg.merchant_address && b.mint == cfg.usdc_mint)
            .try_fold(0u128, |total, balance| {
                total.checked_add(balance.amount).ok_or_else(|| {
                    WatchError::Decode("merchant token balance total overflowed".into())
                })
            })
    };
    let before = sum(&pre)?;
    let after = sum(&post)?;
    let Some(delta) = after.checked_sub(before) else {
        return Ok(Verdict::Mismatch {
            reason: "merchant token balance did not increase".into(),
        });
    };
    if delta != u128::from(expected_units) {
        return Ok(Verdict::Mismatch {
            reason: format!(
                "amount mismatch: credited {delta} base units, expected {expected_units}"
            ),
        });
    }

    Ok(Verdict::Paid {
        payer: transfer.authority,
        signature: signature.to_string(),
        slot,
        payment_block_time_s,
        reference: reference.to_string(),
        item_id: item_id.to_string(),
        amount: expected_amount.to_string(),
        recipient: cfg.merchant_address.clone(),
        mint: cfg.usdc_mint.clone(),
        token_decimals: cfg.token_decimals,
        payment_window_s: cfg.payment_window_s,
    })
}

struct ValidatedTransfer {
    authority: String,
    destination_index: u64,
}

/// Validate the actual on-chain instruction graph, not only its side effects.
/// A valid ProofKiosk payment is compute-budget setup (optional), one exact
/// PKPAY1 memo, then one final transferChecked instruction. The reference must
/// be attached to that transfer and no second reference/transfer is permitted.
fn validate_payment_instructions(
    message: &Value,
    account_keys: &[Value],
    reference: &str,
    item_id: &str,
    mint: &str,
    expected_units: u64,
    token_decimals: u8,
) -> Result<ValidatedTransfer, String> {
    let instructions = message
        .get("instructions")
        .and_then(Value::as_array)
        .ok_or_else(|| "transaction has no parsed instructions".to_string())?;
    let (transfer_ix, setup) = instructions
        .split_last()
        .ok_or_else(|| "transaction has no instructions".to_string())?;

    let mut saw_memo = false;
    for ix in setup {
        let program_id = ix
            .get("programId")
            .and_then(Value::as_str)
            .ok_or_else(|| "instruction has no programId".to_string())?;
        if program_id == COMPUTE_BUDGET_PROGRAM_ID && !saw_memo {
            continue;
        }
        if program_id != MEMO_PROGRAM_ID_B58 || saw_memo {
            return Err(
                "payment may contain only compute-budget setup, one memo, and one transfer".into(),
            );
        }
        if ix.get("program").and_then(Value::as_str) != Some("spl-memo") {
            return Err("memo instruction is not the SPL Memo program".into());
        }
        let memo = ix
            .get("parsed")
            .and_then(Value::as_str)
            .ok_or_else(|| "memo instruction has no parsed text".to_string())?;
        let parsed: Value = serde_json::from_str(memo)
            .map_err(|_| "payment instruction memo is not JSON".to_string())?;
        if !payment_memo_matches(&parsed, reference, item_id) {
            return Err("payment instruction memo does not exactly bind reference and item".into());
        }
        saw_memo = true;
    }
    if !saw_memo {
        return Err("payment transaction has no exact PKPAY1 memo instruction".into());
    }

    let token_program = transfer_ix
        .get("programId")
        .and_then(Value::as_str)
        .ok_or_else(|| "transfer instruction has no programId".to_string())?;
    if !matches!(token_program, TOKEN_PROGRAM_ID | TOKEN_2022_PROGRAM_ID) {
        return Err("final instruction is not SPL Token or Token-2022".into());
    }
    if transfer_ix.pointer("/parsed/type").and_then(Value::as_str) != Some("transferChecked") {
        return Err("final token instruction is not transferChecked".into());
    }
    let info = transfer_ix
        .pointer("/parsed/info")
        .and_then(Value::as_object)
        .ok_or_else(|| "transferChecked has no parsed info".to_string())?;
    let field = |name: &str| {
        info.get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("transferChecked has no `{name}`"))
    };
    if field("mint")? != mint {
        return Err("transferChecked uses a different mint".into());
    }
    let destination = field("destination")?;
    if field("source")? == destination {
        return Err("transferChecked source and destination are identical".into());
    }
    let authority = info
        .get("multisigAuthority")
        .or_else(|| info.get("authority"))
        .and_then(Value::as_str)
        .ok_or_else(|| "transferChecked has no authority".to_string())?;
    if !signers(account_keys).any(|signer| signer == authority) {
        return Err("transfer authority did not sign the transaction".into());
    }
    let references = info
        .get("signers")
        .and_then(Value::as_array)
        .ok_or_else(|| "transferChecked does not attach a reference".to_string())?;
    if references.len() != 1 || references[0].as_str() != Some(reference) {
        return Err("transferChecked must attach exactly this one reference".into());
    }
    let amount = info
        .get("tokenAmount")
        .and_then(|value| value.get("amount"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "transferChecked token amount is invalid".to_string())?;
    let decimals = info
        .get("tokenAmount")
        .and_then(|value| value.get("decimals"))
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| "transferChecked token decimals are invalid".to_string())?;
    if amount != expected_units || decimals != token_decimals {
        return Err(format!(
            "transferChecked amount/decimals mismatch: got {amount} at {decimals}, expected {expected_units} at {token_decimals}"
        ));
    }
    let destination_index = account_keys
        .iter()
        .position(|key| account_key_pubkey(key) == Some(destination))
        .ok_or_else(|| "transfer destination is absent from accountKeys".to_string())?
        as u64;
    Ok(ValidatedTransfer {
        authority: authority.to_string(),
        destination_index,
    })
}

/// Verify an authenticated device heartbeat. Public transactions and memos are
/// ignored; only a successful attestation signed by the configured authority,
/// naming the configured device address and id, may produce `Live`.
pub fn verify_heartbeat<T: RpcTransport>(
    cfg: &WatchConfig,
    transport: T,
    now: u64,
) -> Result<Heartbeat, WatchError> {
    let device = cfg
        .device_address
        .as_deref()
        .filter(|d| b58::decode_pubkey(d).is_some())
        .ok_or_else(|| WatchError::Config("device_address is required for heartbeat".into()))?;
    let device_id = cfg
        .device_id
        .as_deref()
        .ok_or_else(|| WatchError::Config("device_id is required for heartbeat".into()))?;
    let authority = cfg
        .device_authority
        .as_deref()
        .ok_or_else(|| WatchError::Config("device_authority is required for heartbeat".into()))?;
    let max_silence = cfg.heartbeat_max_silence_s;

    let client = RpcClient::new(transport);
    let sigs = client.call(
        "getSignaturesForAddress",
        json!([device, { "commitment": cfg.finality, "limit": SIG_LIMIT }]),
    )?;
    let sig_list = sigs.as_array().ok_or_else(|| {
        WatchError::Decode("getSignaturesForAddress did not return an array".into())
    })?;
    if sig_list.len() > SIG_LIMIT as usize {
        return Err(WatchError::Decode(format!(
            "getSignaturesForAddress exceeded the requested {SIG_LIMIT}-entry limit"
        )));
    }
    for entry in sig_list {
        let signature = entry
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| WatchError::Decode("signature entry missing `signature`".into()))?;
        if entry.get("err").is_some_and(|err| !err.is_null()) {
            continue;
        }
        let txv = fetch_transaction(&client, signature, cfg)?;
        if authority_signed_device_message(&txv, device, authority, None)?.is_none() {
            continue;
        }
        let expected_memo = match entry_memo_payload(entry) {
            Some(memo) => memo,
            None => {
                let is_initialization =
                    chain::is_exact_nonce_initialization_transaction(&txv, device, authority)
                        .map_err(|error| {
                            WatchError::Decode(format!(
                                "authenticated memo-less device transaction is invalid: {error}"
                            ))
                        })?;
                if is_initialization {
                    // Initialization is an incarnation boundary. Older device
                    // attestations belong to a closed/recreated nonce account
                    // and must never revive liveness for the fresh device.
                    return Ok(Heartbeat::Missing);
                }
                return Err(WatchError::Decode(
                    "authenticated heartbeat candidate has no memo".into(),
                ));
            }
        };
        if !authenticated_device_tx(&txv, device, authority, expected_memo, None)? {
            continue;
        }
        let signed_ts =
            chain::attestation_timestamp(expected_memo, device_id).map_err(|error| {
                WatchError::Decode(format!(
                    "authenticated device attestation has an invalid memo schema: {error}"
                ))
            })?;
        let block_time = entry
            .get("blockTime")
            .and_then(Value::as_i64)
            .filter(|time| *time >= 0)
            .map(|time| time as u64)
            .ok_or_else(|| {
                WatchError::Decode("authenticated device attestation has no valid blockTime".into())
            })?;
        if block_time > now.saturating_add(MAX_FUTURE_BLOCKTIME_SKEW_S) {
            return Err(WatchError::Decode(
                "authenticated device attestation blockTime is unreasonably far in the future"
                    .into(),
            ));
        }
        if signed_ts > now.saturating_add(MAX_FUTURE_BLOCKTIME_SKEW_S) {
            return Err(WatchError::Decode(
                "authenticated device attestation signed timestamp is unreasonably far in the future"
                    .into(),
            ));
        }
        if signed_ts > block_time.saturating_add(MAX_FUTURE_BLOCKTIME_SKEW_S) {
            return Err(WatchError::Decode(
                "authenticated device attestation signed timestamp is later than its landing time"
                    .into(),
            ));
        }
        let signature = signature.to_string();
        // Durable-nonce artifacts may remain submit-able for a long time. Use
        // the authenticated host-observation time, not the later landing time,
        // so delayed submission cannot replay an old heartbeat as fresh.
        let age_s = now.saturating_sub(signed_ts);
        return if age_s > max_silence {
            Ok(Heartbeat::Stale { signature, age_s })
        } else {
            Ok(Heartbeat::Live { signature, age_s })
        };
    }
    Ok(Heartbeat::Missing)
}

// ── helpers ──────────────────────────────────────────────────────────────────

struct TokenBalance {
    account_index: u64,
    owner: String,
    mint: String,
    amount: u128,
    /// The mint's decimal count, as the node reports it for this balance. The
    /// only trustworthy source of scale: it is not assumed to be USDC's 6.
    decimals: u8,
}

fn token_balances(
    meta: &Value,
    key: &str,
    account_key_count: usize,
) -> Result<Vec<TokenBalance>, WatchError> {
    let entries = meta
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| WatchError::Decode(format!("transaction meta has no `{key}` array")))?;
    let mut seen = HashSet::new();
    entries
        .iter()
        .map(|balance| {
            let field = |name: &str| {
                balance
                    .get(name)
                    .ok_or_else(|| WatchError::Decode(format!("{key} entry is missing `{name}`")))
            };
            let account_index = field("accountIndex")?
                .as_u64()
                .ok_or_else(|| WatchError::Decode(format!("{key} accountIndex is not u64")))?;
            if account_index >= account_key_count as u64 || !seen.insert(account_index) {
                return Err(WatchError::Decode(format!(
                    "{key} has an out-of-range or duplicate accountIndex"
                )));
            }
            let ui = field("uiTokenAmount")?;
            let amount = ui
                .get("amount")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| WatchError::Decode(format!("{key} amount is not u64")))?;
            let decimals = ui
                .get("decimals")
                .and_then(Value::as_u64)
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| WatchError::Decode(format!("{key} decimals is not u8")))?;
            Ok(TokenBalance {
                account_index,
                owner: field("owner")?
                    .as_str()
                    .ok_or_else(|| WatchError::Decode(format!("{key} owner is not a string")))?
                    .to_string(),
                mint: field("mint")?
                    .as_str()
                    .ok_or_else(|| WatchError::Decode(format!("{key} mint is not a string")))?
                    .to_string(),
                amount: u128::from(amount),
                decimals,
            })
        })
        .collect()
}

/// accountKeys entries are `"pubkey"` (base58) or `{ "pubkey": "..." }`
/// depending on encoding; accept both.
fn account_key_pubkey(v: &Value) -> Option<&str> {
    v.as_str()
        .or_else(|| v.get("pubkey").and_then(Value::as_str))
}
