import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { claimOrder, inspectOrder, validateClaim } from './trusted-order-claim.mjs';

const REFERENCE = 'Stake11111111111111111111111111111111111111';
const SIGNATURE = '1'.repeat(64);
const RECIPIENT = '4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T';
const MINT = 'So11111111111111111111111111111111111111112';
const order = {
  v: 1,
  status: 'created',
  reference: REFERENCE,
  item_id: 'cold_drink',
  amount: '1.5',
  recipient: RECIPIENT,
  mint: MINT,
  token_decimals: 6,
  payment_window_s: 900,
  url: `solana:${RECIPIENT}`,
  created_at_ms: 1_000_000,
  expires_at_ms: 1_900_000,
  raw_output_sha256: 'a'.repeat(64),
};

function watch(overrides = {}) {
  return JSON.stringify({
    v: 1,
    success: true,
    status: 'paid',
    payer: 'Vote111111111111111111111111111111111111111',
    signature: SIGNATURE,
    slot: 42,
    reference: REFERENCE,
    item_id: 'cold_drink',
    amount: '1.5',
    recipient: RECIPIENT,
    mint: MINT,
    token_decimals: 6,
    payment_window_s: 900,
    payment_block_time_s: 1_500,
    payment_verified: true,
    actuation_authorized: false,
    requires_atomic_claim: true,
    message: 'claim required',
    ...overrides,
  });
}

test('validates exact paid output against its persisted order', () => {
  assert.equal(validateClaim(watch(), order).signature, SIGNATURE);
  assert.throws(() => validateClaim(watch({ reference: RECIPIENT }), order));
  assert.throws(() => validateClaim(watch({ status: 'pending', success: false }), order));
  assert.throws(() => validateClaim(`model prose ${watch()}`, order));
});

test('binds the paid result to immutable quoted economics across catalog changes', () => {
  for (const changed of [
    { amount: '0.01' },
    { recipient: MINT },
    { mint: RECIPIENT },
    { token_decimals: 9 },
    { payment_window_s: 60 },
  ]) {
    assert.throws(
      () => validateClaim(watch(changed), order),
      /persisted order economics and policy/u,
    );
  }
});

test('uses payment landing time for quote expiry and permits late outage recovery', () => {
  assert.equal(validateClaim(watch({ payment_block_time_s: 1_900 }), order).signature, SIGNATURE);
  assert.throws(
    () => validateClaim(watch({ payment_block_time_s: 1_901 }), order),
    /quote expired/u,
  );
  assert.throws(
    () => validateClaim(watch({ payment_block_time_s: 900 }), order),
    /predates/u,
  );
});

test('durably claims once and rejects a second actuator attempt', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'proofkiosk-claim-test-'));
  try {
    fs.writeFileSync(path.join(directory, `${REFERENCE}.json`), `${JSON.stringify(order)}\n`, { mode: 0o600 });
    const first = claimOrder({
      reference: REFERENCE,
      ordersDir: directory,
      rawWatchResult: watch(),
      driverId: 'test-driver',
      nowMs: 5_000_000,
    });
    assert.equal(first.status, 'claimed');
    assert.equal(first.claimed_at_ms, 5_000_000);
    assert.equal(first.amount, '1.5');
    assert.equal(first.payment_block_time_s, 1_500);
    assert.throws(() => claimOrder({
      reference: REFERENCE,
      ordersDir: directory,
      rawWatchResult: watch(),
      driverId: 'test-driver',
      nowMs: 5_000_001,
    }), /already claimed/u);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('inspects valid evidence without consuming the one-time claim', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'proofkiosk-inspect-test-'));
  try {
    fs.writeFileSync(path.join(directory, `${REFERENCE}.json`), `${JSON.stringify(order)}\n`, { mode: 0o600 });
    const inspected = inspectOrder({ reference: REFERENCE, ordersDir: directory, rawWatchResult: watch() });
    assert.equal(inspected.watch.status, 'paid');
    assert.equal(inspected.order.item_id, 'cold_drink');
    assert.equal(fs.existsSync(path.join(directory, `${REFERENCE}.claim.json`)), false);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
