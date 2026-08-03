//! Pure charge core. No wasm dependency — compiles and tests on the host.
//!
//! Custody: T1. This module cannot move funds, sign anything, or reach the
//! network. It renders a Solana Pay URL whose recipient is ALWAYS the
//! operator's configured merchant address. The model chooses an item id or a
//! capped free amount — nothing else.

use std::collections::HashMap;

use kiosk_core::{
    b58,
    memo::PAYMENT_TAG,
    pay::{canonicalize_amount, validate_amount, TransferRequest},
    shape,
};
use serde_json::json;

/// Mainnet USDC mint, the shipped default. Operators override in config
/// (e.g. devnet USDC) — never the model.
pub const DEFAULT_USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const DEFAULT_MAX_AMOUNT: &str = "100";
/// Wallet-visible note length, in CHARACTERS. Bounded so a long note cannot
/// crowd the token budget; counted in characters so truncation can never split
/// a UTF-8 code point.
pub const NOTE_MAX_CHARS: usize = 64;
pub const LABEL_MAX_CHARS: usize = 64;
pub const PRICE_LIST_MAX_BYTES: usize = 4_096;
pub const USDC_DECIMALS: u8 = 6;
const MAX_MINT_DECIMALS: u8 = 18;

#[derive(Debug)]
pub struct ChargeConfig {
    pub merchant_address: String,
    pub usdc_mint: String,
    pub token_decimals: u8,
    /// item id -> decimal amount string, parsed from `price_list`
    /// (`"cold_drink:1.5, snack:0.75"`).
    pub price_list: HashMap<String, String>,
    pub max_amount: String,
    pub label: Option<String>,
    /// Optional cosmetic fiat display (e.g. "BRL") shown alongside the USDC
    /// amount. The on-chain amount is ALWAYS the USDC figure; this is a static,
    /// operator-set convenience only (no oracle, no price feed).
    pub display_currency: Option<String>,
    /// Static units-of-`display_currency` per 1 USDC. Only used when
    /// `display_currency` is also set.
    pub display_rate: Option<f64>,
}

#[derive(Debug, PartialEq)]
pub enum ChargeError {
    /// Operator config is missing/invalid — refuse to operate at all.
    Config(String),
    /// Caller arguments rejected (unknown item, over cap, malformed).
    Args(String),
}

impl core::fmt::Display for ChargeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ChargeError::Config(m) => write!(f, "config error: {m}"),
            ChargeError::Args(m) => write!(f, "invalid request: {m}"),
        }
    }
}

impl ChargeConfig {
    /// Build from the flat `string -> string` section the host injects as
    /// `__config`. Fail closed: without a valid merchant address this plugin
    /// refuses to produce anything.
    pub fn from_section(section: &HashMap<String, String>) -> Result<Self, ChargeError> {
        let merchant_address = section
            .get("merchant_address")
            .cloned()
            .ok_or_else(|| ChargeError::Config("merchant_address is required".into()))?;
        if b58::decode_pubkey(&merchant_address).is_none() {
            return Err(ChargeError::Config(
                "merchant_address is not a valid 32-byte base58 pubkey".into(),
            ));
        }
        let usdc_mint = section
            .get("usdc_mint")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| DEFAULT_USDC_MINT.to_string());
        if b58::decode_pubkey(&usdc_mint).is_none() {
            return Err(ChargeError::Config(
                "usdc_mint is not a valid pubkey".into(),
            ));
        }
        let token_decimals = section
            .get("token_decimals")
            .filter(|value| !value.is_empty())
            .map(|value| {
                value.parse::<u8>().map_err(|_| {
                    ChargeError::Config("token_decimals must be an integer from 0 to 18".into())
                })
            })
            .transpose()?
            .unwrap_or(USDC_DECIMALS);
        if token_decimals > MAX_MINT_DECIMALS {
            return Err(ChargeError::Config(format!(
                "token_decimals must be between 0 and {MAX_MINT_DECIMALS}"
            )));
        }
        let max_amount = canonicalize_amount(
            section
                .get("max_amount_usdc")
                .map(String::as_str)
                .unwrap_or(DEFAULT_MAX_AMOUNT),
            token_decimals,
        )
        .map_err(|error| ChargeError::Config(format!("invalid max_amount_usdc: {error}")))?;
        let mut price_list = HashMap::new();
        if let Some(raw) = section.get("price_list") {
            if raw.len() > PRICE_LIST_MAX_BYTES {
                return Err(ChargeError::Config(format!(
                    "price_list exceeds {PRICE_LIST_MAX_BYTES} bytes"
                )));
            }
            for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                let (item, amount) = entry
                    .split_once(':')
                    .ok_or_else(|| ChargeError::Config(format!("bad price entry `{entry}`")))?;
                let item = item.trim();
                if item.is_empty()
                    || item.len() > 64
                    || !item
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(ChargeError::Config(
                        "price-list item ids must use 1 to 64 ASCII letters, digits, `_`, or `-`"
                            .into(),
                    ));
                }
                let amount = validate_amount(amount.trim(), token_decimals, &max_amount).map_err(
                    |error| ChargeError::Config(format!("invalid price for `{item}`: {error}")),
                )?;
                if price_list.insert(item.to_string(), amount).is_some() {
                    return Err(ChargeError::Config(format!(
                        "duplicate price-list item `{item}`"
                    )));
                }
            }
        }
        let display_currency = section
            .get("display_currency")
            .filter(|v| !v.is_empty())
            .cloned();
        let display_rate = match section.get("display_rate").filter(|v| !v.is_empty()) {
            Some(v) => Some(
                v.parse::<f64>()
                    .ok()
                    .filter(|r| r.is_finite() && *r > 0.0)
                    .ok_or_else(|| {
                        ChargeError::Config("display_rate must be a positive number".into())
                    })?,
            ),
            None => None,
        };
        let label = section.get("label").filter(|v| !v.is_empty()).cloned();
        if label
            .as_ref()
            .is_some_and(|value| value.chars().count() > LABEL_MAX_CHARS)
        {
            return Err(ChargeError::Config(format!(
                "label exceeds {LABEL_MAX_CHARS} characters"
            )));
        }
        Ok(Self {
            merchant_address,
            usdc_mint,
            token_decimals,
            price_list,
            max_amount,
            label,
            display_currency,
            display_rate,
        })
    }
}

/// Arguments the model may pass. `deny_unknown_fields` makes smuggled keys
/// (`recipient`, `mint`, …) a hard error — injection drill case #4.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ChargeArgs {
    /// Item id from the operator's price list. **Item-priced charges are the
    /// only actuation-eligible class** — `kiosk_watch` re-derives the amount
    /// from the same `price_list` and can therefore gate a relay on it.
    pub item_id: Option<String>,
    /// Free amount in USDC as a decimal string; only when no item_id, and
    /// always bounded by the operator's `max_amount_usdc`.
    ///
    /// **Invoicing-only.** A free-amount charge carries no item id, so there is
    /// no operator-set price for `kiosk_watch` to verify against; it refuses
    /// such a charge outright rather than accepting a caller-supplied number.
    /// Use this to bill a custom amount a human settles — never to gate
    /// hardware. See `docs-local/DECISIONS.md` (2026-08-02).
    pub amount_usdc: Option<String>,
    /// Short free-text note shown in the wallet (percent-encoded, inert).
    pub note: Option<String>,
}

#[derive(Debug)]
pub struct ChargeOutput {
    pub url: String,
    pub reference: String,
    pub amount: String,
    pub item: Option<String>,
    pub recipient: String,
    pub mint: String,
    pub created_at_ms: u64,
    /// Human/LLM-facing summary, token-budgeted.
    pub summary: String,
}

impl ChargeOutput {
    /// Versioned output for deterministic routing and order handoff. The human
    /// summary remains available as `message`, while every security-relevant
    /// field is separately machine-readable.
    pub fn machine_output(&self) -> String {
        json!({
            "v": 1,
            "success": true,
            "status": "created",
            "actuation_eligible": self.item.is_some(),
            "reference": self.reference,
            "item_id": self.item,
            "amount": self.amount,
            "recipient": self.recipient,
            "mint": self.mint,
            "created_at_ms": self.created_at_ms,
            "url": self.url,
            "message": self.summary,
        })
        .to_string()
    }
}

/// Build a charge. `reference32` and `now_ms` are supplied by the shim (or the
/// test) so this core stays fully deterministic.
pub fn execute_charge(
    args: &ChargeArgs,
    cfg: &ChargeConfig,
    reference32: [u8; 32],
    now_ms: u64,
) -> Result<ChargeOutput, ChargeError> {
    let (amount, item): (String, Option<String>) = match (&args.item_id, &args.amount_usdc) {
        (Some(item), _) => {
            let price = cfg
                .price_list
                .get(item)
                .ok_or_else(|| ChargeError::Args(format!("unknown item `{item}`")))?;
            (price.clone(), Some(item.clone()))
        }
        (None, Some(amount)) => (amount.clone(), None),
        (None, None) => return Err(ChargeError::Args("provide item_id or amount_usdc".into())),
    };

    let reference = b58::encode(&reference32);
    // Count characters, not bytes. `String::truncate` panics when the byte
    // index lands inside a multi-byte character, so a customer typing emoji
    // would take down a plugin that gates hardware.
    let note = args
        .note
        .as_deref()
        .map(|n| n.chars().take(NOTE_MAX_CHARS).collect::<String>());
    // Bind the on-chain transfer to both this unguessable reference and the
    // quoted item. kiosk-watch requires this exact versioned claim, so one
    // payment carrying several reference accounts cannot satisfy several
    // same-priced orders, and an equal-priced SKU cannot be swapped at watch.
    let payment_memo = item.as_ref().map(|item_id| {
        json!({
            "v": 1,
            "tag": PAYMENT_TAG,
            "ref": reference,
            "item": item_id,
        })
        .to_string()
    });
    let request = TransferRequest::new(
        &cfg.merchant_address,
        &amount,
        cfg.token_decimals,
        &cfg.max_amount,
        Some(&cfg.usdc_mint),
        Some(&reference),
        cfg.label.as_deref(),
        note.as_deref(),
        payment_memo.as_deref(),
    )
    .map_err(|e| ChargeError::Args(e.to_string()))?;

    let url = request.url();
    let what = item.clone().unwrap_or_else(|| "custom amount".to_string());
    // Optional cosmetic fiat hint (e.g. "≈ BRL 7.50"). Never changes the
    // on-chain amount — that is always the USDC figure above.
    let fiat = match (&cfg.display_currency, cfg.display_rate) {
        (Some(cur), Some(rate)) => amount
            .parse::<f64>()
            .ok()
            .map(|a| format!(" (≈ {cur} {:.2})", a * rate))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let summary = shape::clamp(
        &format!(
            "Charge created: {amount} USDC{fiat} for `{what}`. Show this Solana Pay link/QR to the customer. Reference for payment-watch: {reference}. URL: {url}"
        ),
        shape::DEFAULT_BUDGET_TOKENS,
    );

    Ok(ChargeOutput {
        url,
        reference,
        amount,
        item,
        recipient: cfg.merchant_address.clone(),
        mint: cfg.usdc_mint.clone(),
        created_at_ms: now_ms,
        summary,
    })
}
