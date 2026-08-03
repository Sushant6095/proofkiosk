//! Host-run tests for the kiosk-charge core, driven exactly as the wasm shim
//! drives it: config from a flat section, strict args, deterministic reference.
//! Plain `cargo test` — no wasm toolchain, no network (this plugin HAS no
//! network), RPC not involved at all.

use std::collections::HashMap;

use kiosk_charge::charge::{
    execute_charge, ChargeArgs, ChargeConfig, ChargeError, DEFAULT_USDC_MINT,
};

const MERCHANT: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T"; // arbitrary valid pk
const REF32: [u8; 32] = [7u8; 32];

fn section(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn base_cfg() -> ChargeConfig {
    ChargeConfig::from_section(&section(&[
        ("merchant_address", MERCHANT),
        ("price_list", "cold_drink:1.5, snack:0.75"),
        ("max_amount_usdc", "10"),
        ("label", "Kiosk 01"),
    ]))
    .unwrap()
}

#[test]
fn item_charge_builds_solana_pay_url() {
    let out = execute_charge(
        &ChargeArgs {
            item_id: Some("cold_drink".into()),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap();
    assert!(out
        .url
        .starts_with(&format!("solana:{MERCHANT}?amount=1.5")));
    assert!(out.url.contains(&format!("spl-token={DEFAULT_USDC_MINT}")));
    assert!(out.url.contains(&format!("reference={}", out.reference)));
    assert!(out.url.contains("label=Kiosk%2001"));
    assert!(out.url.contains("memo=%7B"));
    assert!(out.url.contains("PKPAY1"));
    assert!(out.url.contains(&out.reference));
    assert_eq!(out.amount, "1.5");
    assert_eq!(out.created_at_ms, 0);
    let machine: serde_json::Value = serde_json::from_str(&out.machine_output()).unwrap();
    assert_eq!(machine["v"], 1);
    assert_eq!(machine["status"], "created");
    assert_eq!(machine["actuation_eligible"], true);
    assert_eq!(machine["reference"], out.reference);
    assert_eq!(machine["item_id"], "cold_drink");
    assert_eq!(machine["amount"], "1.5");
    assert_eq!(machine["recipient"], MERCHANT);
    assert_eq!(machine["mint"], DEFAULT_USDC_MINT);
    assert_eq!(machine["url"], out.url);
}

#[test]
fn generalizes_to_any_spl_mint_not_just_usdc() {
    // BROADLY-USABLE: the mint is operator config, so any stablecoin/token works.
    let other_mint = "So11111111111111111111111111111111111111112";
    let cfg = ChargeConfig::from_section(&section(&[
        ("merchant_address", MERCHANT),
        ("usdc_mint", other_mint),
        ("price_list", "cold_drink:1.5"),
    ]))
    .unwrap();
    let out = execute_charge(
        &ChargeArgs {
            item_id: Some("cold_drink".into()),
            ..Default::default()
        },
        &cfg,
        REF32,
        0,
    )
    .unwrap();
    assert!(out.url.contains(&format!("spl-token={other_mint}")));
}

#[test]
fn non_six_decimal_mint_is_validated_and_rendered_canonically() {
    let cfg = ChargeConfig::from_section(&section(&[
        ("merchant_address", MERCHANT),
        ("usdc_mint", "So11111111111111111111111111111111111111112"),
        ("token_decimals", "9"),
        ("price_list", "credit:001.500000000, dust:0.000000001"),
    ]))
    .unwrap();
    assert_eq!(cfg.token_decimals, 9);
    let out = execute_charge(
        &ChargeArgs {
            item_id: Some("credit".into()),
            ..Default::default()
        },
        &cfg,
        REF32,
        0,
    )
    .unwrap();
    assert!(out.url.contains("amount=1.5"), "{}", out.url);
}

#[test]
fn invalid_decimal_policy_or_catalog_price_fails_at_config_load() {
    for pairs in [
        vec![("merchant_address", MERCHANT), ("token_decimals", "19")],
        vec![("merchant_address", MERCHANT), ("token_decimals", "wat")],
        vec![
            ("merchant_address", MERCHANT),
            ("token_decimals", "2"),
            ("price_list", "item:1.001"),
        ],
        vec![
            ("merchant_address", MERCHANT),
            ("max_amount_usdc", "10"),
            ("price_list", "item:11"),
        ],
        vec![("merchant_address", MERCHANT), ("price_list", "item:.5")],
        vec![("merchant_address", MERCHANT), ("price_list", "item:1.")],
        vec![
            ("merchant_address", MERCHANT),
            ("token_decimals", "18"),
            ("price_list", "item:100"),
        ],
    ] {
        assert!(ChargeConfig::from_section(&section(&pairs)).is_err());
    }
}

#[test]
fn optional_fiat_display_is_cosmetic_only() {
    // display_currency + static rate → a "≈ BRL x.xx" hint in the summary.
    // The on-chain amount stays the USDC figure.
    let cfg = ChargeConfig::from_section(&section(&[
        ("merchant_address", MERCHANT),
        ("price_list", "cold_drink:1.5"),
        ("display_currency", "BRL"),
        ("display_rate", "5.00"),
    ]))
    .unwrap();
    let out = execute_charge(
        &ChargeArgs {
            item_id: Some("cold_drink".into()),
            ..Default::default()
        },
        &cfg,
        REF32,
        0,
    )
    .unwrap();
    assert!(
        out.summary.contains("≈ BRL 7.50"),
        "summary: {}",
        out.summary
    );
    assert!(out.url.contains("amount=1.5"), "on-chain amount stays USDC");
    // Off by default: base_cfg has no display currency.
    let plain = execute_charge(
        &ChargeArgs {
            item_id: Some("cold_drink".into()),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap();
    assert!(
        !plain.summary.contains("≈"),
        "no fiat hint unless configured"
    );
}

#[test]
fn free_amount_within_cap_ok() {
    let out = execute_charge(
        &ChargeArgs {
            amount_usdc: Some("2.25".into()),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap();
    assert!(out.url.contains("amount=2.25"));
    assert!(out.item.is_none());
    let machine: serde_json::Value = serde_json::from_str(&out.machine_output()).unwrap();
    assert_eq!(machine["actuation_eligible"], false);
    assert!(machine["item_id"].is_null());
}

#[test]
fn duplicate_or_unbounded_operator_config_is_rejected() {
    for pairs in [
        vec![("merchant_address", MERCHANT), ("price_list", "a:1,a:2")],
        vec![("merchant_address", MERCHANT), ("label", &"x".repeat(65))],
        vec![("merchant_address", MERCHANT), ("max_amount_usdc", "inf")],
        vec![("merchant_address", MERCHANT), ("max_amount_usdc", ".5")],
    ] {
        assert!(ChargeConfig::from_section(&section(&pairs)).is_err());
    }
}

#[test]
fn shim_derives_the_reference_from_a_csprng_not_process_state() {
    // The host builds a FRESH wasm store for every execute, so a `static`
    // counter is always 0 on entry: the old reference was really just a
    // millisecond timestamp. Two charges in the same millisecond collided onto
    // one reference — and one payment clears both — while a predictable
    // reference lets an attacker write junk at a charge that does not exist
    // yet. The constitution's "no static/thread_local" rule is what forbids
    // the counter; this guards against it coming back.
    let shim = include_str!("../src/lib.rs");
    assert!(
        !shim.contains("AtomicU64"),
        "the shim must not carry process-local state; a fresh store resets it"
    );
    assert!(
        shim.contains("getrandom"),
        "the reference must come from the host CSPRNG"
    );
}

#[test]
fn distinct_references_produce_distinct_charges() {
    // The core is deliberately deterministic — the reference is an argument —
    // so this pins the contract the shim's randomness rides on.
    let args = || ChargeArgs {
        item_id: Some("cold_drink".into()),
        ..Default::default()
    };
    let a = execute_charge(&args(), &base_cfg(), [1u8; 32], 0).unwrap();
    let b = execute_charge(&args(), &base_cfg(), [2u8; 32], 0).unwrap();
    assert_ne!(a.reference, b.reference);
    assert_ne!(a.url, b.url);
}

#[test]
fn multibyte_note_does_not_panic() {
    // The note is free text from chat, so it can contain any UTF-8. Truncating
    // it by BYTE index lands mid-character and panics — a customer typing emoji
    // would take the plugin down, which on a machine that actuates is a denial
    // of service, not a cosmetic bug. 30 × 3-byte chars = 90 bytes, so byte 64
    // falls inside a character.
    let out = execute_charge(
        &ChargeArgs {
            amount_usdc: Some("2.25".into()),
            note: Some("☕".repeat(30)),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .expect("a multibyte note must not panic or fail");
    assert!(out.url.contains("amount=2.25"));
}

#[test]
fn multibyte_note_truncates_on_a_character_boundary() {
    // Truncation must count characters, not bytes: 100 four-byte emoji must
    // survive as 64 whole characters, never a split code point.
    let out = execute_charge(
        &ChargeArgs {
            amount_usdc: Some("2.25".into()),
            note: Some("🥤".repeat(100)),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap();
    // Percent-encoded in the URL, so assert on the decoded count via the memo
    // parameter length: 64 chars × 4 bytes × 3 chars per %XX escape.
    let encoded = out
        .url
        .split("message=")
        .nth(1)
        .or_else(|| out.url.split("memo=").nth(1))
        .unwrap_or("");
    let escapes = encoded.matches('%').count();
    assert_eq!(
        escapes,
        64 * 4,
        "expected exactly 64 whole 4-byte characters"
    );
}

#[test]
fn output_stays_within_token_budget() {
    let out = execute_charge(
        &ChargeArgs {
            amount_usdc: Some("2.25".into()),
            note: Some("x".repeat(500)),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap();
    assert!(
        kiosk_core::shape::approx_tokens(&out.summary) <= kiosk_core::shape::DEFAULT_BUDGET_TOKENS
    );
}

// ── Fail-closed drill ────────────────────────────────────────────────────────

#[test]
fn injection_unknown_item_rejected() {
    let err = execute_charge(
        &ChargeArgs {
            item_id: Some("free_everything".into()),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap_err();
    assert!(matches!(err, ChargeError::Args(_)));
}

#[test]
fn injection_amount_over_cap_rejected() {
    for bad in ["11", "9999", "10.000001"] {
        let err = execute_charge(
            &ChargeArgs {
                amount_usdc: Some(bad.into()),
                ..Default::default()
            },
            &base_cfg(),
            REF32,
            0,
        )
        .unwrap_err();
        assert!(
            matches!(err, ChargeError::Args(_)),
            "{bad} must be rejected"
        );
    }
}

#[test]
fn injection_smuggled_recipient_key_is_a_serde_error() {
    // The model-facing args struct denies unknown fields: a smuggled
    // `recipient` never reaches charge logic.
    let raw = r#"{"item_id":"cold_drink","recipient":"EvilPubkey111111111111111111111111"}"#;
    let parsed: Result<ChargeArgs, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "unknown `recipient` field must fail deserialization"
    );
}

#[test]
fn injection_note_cannot_forge_url_params() {
    let out = execute_charge(
        &ChargeArgs {
            amount_usdc: Some("1".into()),
            note: Some("&amount=999&recipient=EVIL".into()),
            ..Default::default()
        },
        &base_cfg(),
        REF32,
        0,
    )
    .unwrap();
    assert_eq!(out.url.matches("amount=").count(), 1);
    assert!(!out.url.contains("&recipient="));
}

#[test]
fn missing_merchant_config_fails_closed() {
    let err = ChargeConfig::from_section(&section(&[("price_list", "a:1")])).unwrap_err();
    assert!(matches!(err, ChargeError::Config(_)));
}

#[test]
fn invalid_merchant_pubkey_fails_closed() {
    let err =
        ChargeConfig::from_section(&section(&[("merchant_address", "not-a-key")])).unwrap_err();
    assert!(matches!(err, ChargeError::Config(_)));
}

#[test]
fn no_item_and_no_amount_rejected() {
    let err = execute_charge(&ChargeArgs::default(), &base_cfg(), REF32, 0).unwrap_err();
    assert!(matches!(err, ChargeError::Args(_)));
}
