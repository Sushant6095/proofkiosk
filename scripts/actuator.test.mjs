import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const ROOT = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const ACTUATOR = path.join(ROOT, 'scripts/actuator.mjs');
const REFERENCE = 'Stake11111111111111111111111111111111111111';
const RECIPIENT = '4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T';
const MINT = 'So11111111111111111111111111111111111111112';

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'proofkiosk-actuator-test-'));
  const orders = path.join(root, '.proofkiosk/orders');
  fs.mkdirSync(orders, { recursive: true });
  const order = {
    v: 1, status: 'created', reference: REFERENCE, item_id: 'cold_drink', amount: '1.5',
    recipient: RECIPIENT, mint: MINT, token_decimals: 6, payment_window_s: 900,
    url: `solana:${RECIPIENT}`, created_at_ms: 1_000_000, expires_at_ms: 1_900_000,
    raw_output_sha256: 'a'.repeat(64),
  };
  const watch = {
    v: 1, success: true, status: 'paid', payer: 'Vote111111111111111111111111111111111111111',
    signature: '1'.repeat(64), slot: 42, reference: REFERENCE, item_id: 'cold_drink', amount: '1.5',
    recipient: RECIPIENT, mint: MINT, token_decimals: 6, payment_window_s: 900,
    payment_block_time_s: 1_500, payment_verified: true, actuation_authorized: false,
    requires_atomic_claim: true, message: 'claim required',
  };
  fs.writeFileSync(path.join(orders, `${REFERENCE}.json`), `${JSON.stringify(order)}\n`, { mode: 0o600 });
  const watchFile = path.join(root, 'watch.json');
  fs.writeFileSync(watchFile, JSON.stringify({ success: true, output: JSON.stringify(watch), error: null }));
  return { root, orders, watchFile };
}

function run({ root, orders, watchFile }, extra = [], env = {}) {
  return spawnSync(process.execPath, [ACTUATOR, '--root', root, '--orders-dir', orders,
    '--reference', REFERENCE, '--watch-result', watchFile, ...extra], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
}

test('dry-run validates without touching GPIO or consuming the claim', () => {
  const f = fixture();
  try {
    const result = run(f, ['--dry-run']);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /validated and simulated/u);
    assert.equal(fs.existsSync(path.join(f.orders, `${REFERENCE}.claim.json`)), false);
    assert.equal(fs.existsSync(path.join(f.root, '.proofkiosk/actuator.lock')), false);
    const journal = fs.readFileSync(path.join(f.root, '.proofkiosk/actuator-journal.jsonl'), 'utf8');
    assert.match(journal, /"event":"simulated"/u);
    assert.doesNotMatch(journal, /"event":"delivered"/u);
  } finally {
    fs.rmSync(f.root, { recursive: true, force: true });
  }
});

test('cooldown refusal happens before the one-time claim is consumed', () => {
  const f = fixture();
  try {
    fs.mkdirSync(path.join(f.root, '.proofkiosk'), { recursive: true });
    fs.writeFileSync(path.join(f.root, '.proofkiosk/actuator-journal.jsonl'),
      `${JSON.stringify({ event: 'pulse_completed', at_ms: Date.now() })}\n`);
    const result = run(f, ['--dry-run']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /cooldown active/u);
    assert.equal(fs.existsSync(path.join(f.orders, `${REFERENCE}.claim.json`)), false);
    assert.equal(fs.existsSync(path.join(f.root, '.proofkiosk/actuator.lock')), false);
  } finally {
    fs.rmSync(f.root, { recursive: true, force: true });
  }
});

test('an existing host lock refuses before validation or claim', () => {
  const f = fixture();
  try {
    fs.mkdirSync(path.join(f.root, '.proofkiosk'), { recursive: true });
    fs.writeFileSync(path.join(f.root, '.proofkiosk/actuator.lock'), '{"pid":999}\n');
    const result = run(f, ['--dry-run']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /host lock/u);
    assert.equal(fs.existsSync(path.join(f.orders, `${REFERENCE}.claim.json`)), false);
  } finally {
    fs.rmSync(f.root, { recursive: true, force: true });
  }
});

test('real driver path pulses low-high-low, records pulse completion, and cannot replay', () => {
  const f = fixture();
  try {
    const bin = path.join(f.root, 'bin');
    const gpioLog = path.join(f.root, 'gpio.log');
    fs.mkdirSync(bin);
    const fake = path.join(bin, 'pinctrl');
    fs.writeFileSync(fake, '#!/bin/sh\nprintf "%s\\n" "$*" >> "$GPIO_LOG"\n', { mode: 0o700 });

    const env = { PATH: `${bin}:${process.env.PATH}`, GPIO_LOG: gpioLog };
    const first = run(f, [], env);
    assert.equal(first.status, 0, first.stderr);
    assert.match(first.stdout, /pulse_completed/u);
    assert.deepEqual(fs.readFileSync(gpioLog, 'utf8').trim().split('\n'), [
      'set 17 op dl', 'set 17 op dh', 'set 17 op dl',
    ]);
    assert.equal(fs.existsSync(path.join(f.orders, `${REFERENCE}.claim.json`)), true);

    const journalFile = path.join(f.root, '.proofkiosk/actuator-journal.jsonl');
    const entries = fs.readFileSync(journalFile, 'utf8').trim().split('\n').map(JSON.parse);
    assert.deepEqual(entries.map((entry) => entry.event), ['actuating', 'pulse_completed']);
    assert.equal(entries.some((entry) => entry.event === 'delivered'), false);

    fs.writeFileSync(journalFile, `${entries.map((entry) => JSON.stringify({ ...entry, at_ms: 0 })).join('\n')}\n`);
    const second = run(f, [], env);
    assert.notEqual(second.status, 0);
    assert.match(second.stderr, /already claimed/u);
    assert.equal(fs.readFileSync(gpioLog, 'utf8').trim().split('\n').length, 4,
      'the replay may preflight LOW but must never energize HIGH');
  } finally {
    fs.rmSync(f.root, { recursive: true, force: true });
  }
});
