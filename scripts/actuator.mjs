#!/usr/bin/env node
/**
 * ProofKiosk actuator — the only thing in this repository that touches a pin.
 *
 * It runs on the device, OUTSIDE the agent and outside the WASM sandbox, and it
 * is deliberately not a tool the model can call. Its input is not a request; it
 * is evidence: the raw WIT ToolResult kiosk-watch produced, plus the durable
 * order record. It fires only if the trusted claim layer grants an exclusive,
 * first-and-only claim on that order.
 *
 * The plugin never authorises actuation on its own — a verified payment comes
 * back with `actuation_authorized: false` and `requires_atomic_claim: true`.
 * Verifying that money arrived and deciding to move hardware are two different
 * decisions, made in two different places, and only the second one can fire.
 *
 *   node scripts/actuator.mjs --reference <ref> --watch-result <file> \
 *        [--orders-dir .proofkiosk/orders] [--dry-run]
 *
 * Exit 0 = pin pulsed. Any non-zero = nothing moved.
 */

import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { claimOrder } from './trusted-order-claim.mjs';

// ── Fixed policy. Deliberately NOT configurable from any input the model or a
// payer can influence: a caller that could choose the pin or the pulse length
// would be choosing what the machine does, which is the whole thing we refuse.
const PIN_BCM = 17;
const PULSE_MS = 400;
const MAX_PULSE_MS = 1_000;   // hard ceiling; a bug cannot hold a solenoid on
const COOLDOWN_MS = 3_000;    // floor between physical actions on this host
const JOURNAL = '.proofkiosk/actuator-journal.jsonl';

const args = process.argv.slice(2);
const arg = (flag, fallback = null) => {
  const i = args.indexOf(flag);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const has = (flag) => args.includes(flag);

const die = (msg, code = 1) => {
  process.stderr.write(`actuator: ${msg}\n`);
  process.exit(code);
};

/** Append-only physical record. Written before AND after the pulse, so a crash
 *  mid-actuation is visible as an `actuating` entry with no `delivered`. */
function journal(root, entry) {
  const file = path.join(root, JOURNAL);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.appendFileSync(file, `${JSON.stringify({ ...entry, at_ms: Date.now() })}\n`, { mode: 0o600 });
}

function lastPulseMs(root) {
  const file = path.join(root, JOURNAL);
  if (!fs.existsSync(file)) return 0;
  const lines = fs.readFileSync(file, 'utf8').trim().split('\n').filter(Boolean);
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    try {
      const e = JSON.parse(lines[i]);
      if (e.event === 'delivered') return e.at_ms ?? 0;
    } catch { /* skip malformed */ }
  }
  return 0;
}

/** Drive the pin via `pinctrl` (Pi OS Bookworm) or `raspi-gpio` (older).
 *  Absent both, this is not a Pi and we refuse rather than pretend. */
function gpio(level, dry) {
  if (dry) { process.stdout.write(`  [dry-run] GPIO${PIN_BCM} -> ${level}\n`); return 'dry-run'; }
  for (const [bin, argv] of [
    ['pinctrl', ['set', String(PIN_BCM), 'op', level === 'high' ? 'dh' : 'dl']],
    ['raspi-gpio', ['set', String(PIN_BCM), 'op', level === 'high' ? 'dh' : 'dl']],
  ]) {
    try { execFileSync(bin, argv, { stdio: 'pipe' }); return bin; } catch { /* try next */ }
  }
  die('no GPIO tool found (pinctrl / raspi-gpio). Run this on the Pi, or pass --dry-run.');
}

const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// ── main ─────────────────────────────────────────────────────────────────────
const root = path.resolve(arg('--root', process.cwd()));
const reference = arg('--reference') ?? die('--reference is required');
const watchFile = arg('--watch-result') ?? die('--watch-result is required');
const ordersDir = path.resolve(arg('--orders-dir', path.join(root, '.proofkiosk/orders')));
const dry = has('--dry-run');

const rawWatchResult = fs.readFileSync(watchFile, 'utf8');

// 1. The claim layer is the authorisation. It re-validates the raw ToolResult,
//    checks the persisted order's immutable economics and quote time, and
//    creates the claim with an exclusive O_EXCL write. A second run cannot pass
//    here, which is what makes delivery at-most-once on this host.
let claim;
try {
  // Driver id is an opaque identity for the claim record; the claim layer
  // restricts it to [A-Za-z0-9_-] so it can never smuggle a path separator.
  claim = claimOrder({ reference, ordersDir, rawWatchResult, driverId: 'proofkiosk-actuator-1' });
} catch (e) {
  journal(root, { event: 'refused', reference, reason: e.message });
  die(`claim refused — NOT firing: ${e.message}`);
}

// 2. Cooldown is a physical property of the machine, not of the order.
const since = Date.now() - lastPulseMs(root);
if (since < COOLDOWN_MS) {
  journal(root, { event: 'refused', reference, reason: `cooldown ${COOLDOWN_MS - since}ms remaining` });
  die(`cooldown active (${COOLDOWN_MS - since}ms) — NOT firing`);
}

const pulse = Math.min(PULSE_MS, MAX_PULSE_MS);
process.stdout.write(`claim ${claim.claim_id} granted for ${reference}\n`);
process.stdout.write(`firing GPIO${PIN_BCM} for ${pulse}ms\n`);

// 3. Journal the intent BEFORE moving anything, so a crash during actuation is
//    detectable afterwards as an unfinished delivery rather than silence.
journal(root, { event: 'actuating', reference, claim_id: claim.claim_id, pin: PIN_BCM, pulse_ms: pulse });

let driver = 'unknown';
try {
  gpio('low', dry);            // boot-safe: known state before energising
  driver = gpio('high', dry);
  sleep(pulse);
} finally {
  gpio('low', dry);            // always de-energise, even if the above threw
}

journal(root, { event: 'delivered', reference, claim_id: claim.claim_id, pin: PIN_BCM, pulse_ms: pulse, driver });
process.stdout.write(`delivered: ${reference} (${driver})\n`);
