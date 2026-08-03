import assert from 'node:assert/strict';
import test from 'node:test';

import { validateHandoff } from './trusted-charge-handoff.mjs';

const RECIPIENT = '4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T';
const MINT = 'So11111111111111111111111111111111111111112';
const REFERENCE = 'Stake11111111111111111111111111111111111111';
const CONFIG = `
[[plugins.entries]]
name = "kiosk-charge"
[plugins.entries.config]
merchant_address = "${RECIPIENT}"
usdc_mint = "${MINT}"
token_decimals = "6"
price_list = "cold_drink:1.5, snack:0.75"

[[plugins.entries]]
name = "kiosk-watch"
[plugins.entries.config]
merchant_address = "${RECIPIENT}"
usdc_mint = "${MINT}"
token_decimals = "6"
price_list = "snack:0.75,cold_drink:1.500000"
`;

function machine(overrides = {}) {
  const memo = JSON.stringify({ v: 1, tag: 'PKPAY1', ref: REFERENCE, item: 'cold_drink' });
  const params = new URLSearchParams({
    amount: '1.5',
    'spl-token': MINT,
    reference: REFERENCE,
    memo,
  });
  return {
    v: 1,
    success: true,
    status: 'created',
    actuation_eligible: true,
    reference: REFERENCE,
    item_id: 'cold_drink',
    amount: '1.5',
    recipient: RECIPIENT,
    mint: MINT,
    created_at_ms: 1_000_000,
    url: `solana:${RECIPIENT}?${params}`,
    message: 'trusted display text',
    ...overrides,
  };
}

test('accepts an exact raw catalog charge and canonicalizes its amount', () => {
  const result = validateHandoff(JSON.stringify(machine({ amount: '01.500000' })), CONFIG);
  assert.equal(result.amount, '1.5');
  assert.equal(result.recipient, RECIPIENT);
  assert.equal(result.token_decimals, 6);
  assert.equal(result.payment_window_s, 900);
  assert.equal(result.expires_at_ms, 1_900_000);
});

test('accepts an exact WIT ToolResult wrapper, not prose', () => {
  const wrapped = JSON.stringify({ success: true, output: JSON.stringify(machine()), error: null });
  assert.equal(validateHandoff(wrapped, CONFIG).reference, REFERENCE);
  assert.throws(() => validateHandoff(`Agent says: ${JSON.stringify(machine())}`, CONFIG));
});

test('rejects a model-substituted recipient even when it still uses solana scheme', () => {
  const attacker = 'Vote111111111111111111111111111111111111111';
  assert.throws(() => validateHandoff(JSON.stringify(machine({ recipient: attacker })), CONFIG));
  const tampered = machine();
  tampered.url = tampered.url.replace(RECIPIENT, attacker);
  assert.throws(() => validateHandoff(JSON.stringify(tampered), CONFIG));
});

test('rejects amount, item, mint, reference, and memo drift', () => {
  for (const changed of [
    { amount: '0.01' },
    { item_id: 'unknown' },
    { mint: RECIPIENT },
    { reference: RECIPIENT },
  ]) {
    assert.throws(() => validateHandoff(JSON.stringify(machine(changed)), CONFIG));
  }
  const tamperedMemo = machine();
  const url = new URL(tamperedMemo.url);
  url.searchParams.set('memo', JSON.stringify({ v: 1, tag: 'PKPAY1', ref: RECIPIENT, item: 'cold_drink' }));
  tamperedMemo.url = url.toString();
  assert.throws(() => validateHandoff(JSON.stringify(tamperedMemo), CONFIG));
});

test('rejects duplicate security parameters and unknown URL fields', () => {
  const duplicate = machine();
  duplicate.url += `&reference=${encodeURIComponent(RECIPIENT)}`;
  assert.throws(() => validateHandoff(JSON.stringify(duplicate), CONFIG));
  const unknown = machine();
  unknown.url += '&recipient=attacker';
  assert.throws(() => validateHandoff(JSON.stringify(unknown), CONFIG));
});

test('rejects charge/watch configuration drift and encrypted unverifiable values', () => {
  assert.throws(() => validateHandoff(JSON.stringify(machine()), CONFIG.replace(
    'price_list = "snack:0.75,cold_drink:1.500000"',
    'price_list = "snack:0.75,cold_drink:1.4"',
  )));
  assert.throws(() => validateHandoff(JSON.stringify(machine()), CONFIG.replace(
    `merchant_address = "${RECIPIENT}"`,
    'merchant_address = "enc2:ciphertext"',
  )));
});

test('rejects catalog amounts that cannot fit an SPL u64 amount', () => {
  const huge = CONFIG
    .replaceAll('token_decimals = "6"', 'token_decimals = "18"')
    .replaceAll('cold_drink:1.5', 'cold_drink:100');
  assert.throws(
    () => validateHandoff(JSON.stringify(machine()), huge),
    /u64 token amount/u,
  );
});

test('rejects invalid quote windows and timestamp overflow', () => {
  assert.throws(() => validateHandoff(JSON.stringify(machine()), `${CONFIG}\npayment_window_s = "0"\n`));
  const explicitWindow = CONFIG.replace(
    'price_list = "snack:0.75,cold_drink:1.500000"',
    'price_list = "snack:0.75,cold_drink:1.500000"\npayment_window_s = "60"',
  );
  assert.equal(
    validateHandoff(JSON.stringify(machine()), explicitWindow).expires_at_ms,
    1_060_000,
  );
  assert.throws(() => validateHandoff(
    JSON.stringify(machine({ created_at_ms: Number.MAX_SAFE_INTEGER })),
    explicitWindow,
  ));
});
