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
 * Exit 0 = evidence validated and either a real pulse completed or `--dry-run`
 * simulated it. Any non-zero = the driver did not intentionally energise the pin.
 */

import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { claimOrder, inspectOrder } from './trusted-order-claim.mjs';

// ── Fixed policy. Deliberately NOT configurable from any input the model or a
// payer can influence: a caller that could choose the pin or the pulse length
// would be choosing what the machine does, which is the whole thing we refuse.
const PIN_BCM = 17;
const PULSE_MS = 400;
const MAX_PULSE_MS = 1_000;   // hard ceiling; a bug cannot hold a solenoid on
const COOLDOWN_MS = 3_000;    // floor between physical actions on this host
const JOURNAL = '.proofkiosk/actuator-journal.jsonl';
const LOCK = '.proofkiosk/actuator.lock';
const GPIO_COMMAND_TIMEOUT_MS = 1_500;

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
 *  mid-actuation is visible as an `actuating` entry with no `pulse_completed`. */
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
      if (e.event === 'pulse_completed' || e.event === 'delivered') return e.at_ms ?? 0;
    } catch { /* skip malformed */ }
  }
  return 0;
}

/** Drive the pin via `pinctrl` (Pi OS Bookworm) or `raspi-gpio` (older).
 *  Absent both, this is not a Pi and we refuse rather than pretend. */
function gpio(level, dry) {
  if (dry) { process.stdout.write(`  [dry-run] GPIO${PIN_BCM} -> ${level}\n`); return 'dry-run'; }
  const failures = [];
  for (const [bin, argv] of [
    ['pinctrl', ['set', String(PIN_BCM), 'op', level === 'high' ? 'dh' : 'dl']],
    ['raspi-gpio', ['set', String(PIN_BCM), 'op', level === 'high' ? 'dh' : 'dl']],
  ]) {
    try {
      execFileSync(bin, argv, { stdio: 'pipe', timeout: GPIO_COMMAND_TIMEOUT_MS });
      return bin;
    } catch (error) {
      failures.push(`${bin}:${error?.code ?? error?.signal ?? 'failed'}`);
    }
  }
  throw new Error(`GPIO command failed (${failures.join(', ')}). Run this on the Pi, or pass --dry-run.`);
}

const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// ── main ─────────────────────────────────────────────────────────────────────
const root = path.resolve(arg('--root', process.cwd()));
const reference = arg('--reference') ?? die('--reference is required');
const watchFile = arg('--watch-result') ?? die('--watch-result is required');
const ordersDir = path.resolve(arg('--orders-dir', path.join(root, '.proofkiosk/orders')));
const dry = has('--dry-run');

const rawWatchResult = fs.readFileSync(watchFile, 'utf8');
const lockFile = path.join(root, LOCK);
fs.mkdirSync(path.dirname(lockFile), { recursive: true });
let lockFd;
try {
  lockFd = fs.openSync(lockFile, 'wx', 0o600);
  fs.writeFileSync(lockFd, `${JSON.stringify({ pid: process.pid, reference, acquired_at_ms: Date.now() })}\n`);
  fs.fsyncSync(lockFd);
} catch (error) {
  if (error?.code === 'EEXIST') die('another actuator run holds the host lock — NOT firing');
  die(`cannot acquire actuator lock — NOT firing: ${error.message}`);
}

try {
  // 1. Cooldown is checked while holding the host-wide lock and BEFORE the
  // one-time order claim is consumed. A refused cooldown remains retryable.
  const since = Date.now() - lastPulseMs(root);
  if (since < COOLDOWN_MS) {
    const remaining = COOLDOWN_MS - since;
    journal(root, { event: 'refused', reference, reason: `cooldown ${remaining}ms remaining` });
    throw new Error(`cooldown active (${remaining}ms) — NOT firing`);
  }

  const pulse = Math.min(PULSE_MS, MAX_PULSE_MS);

  if (dry) {
    inspectOrder({ reference, ordersDir, rawWatchResult });
    gpio('low', true);
    gpio('high', true);
    sleep(pulse);
    gpio('low', true);
    journal(root, { event: 'simulated', reference, pin: PIN_BCM, pulse_ms: pulse });
    process.stdout.write(`validated and simulated: ${reference}; no claim created, no pin touched\n`);
    process.exitCode = 0;
  } else {
    // 2. Prove the OS GPIO boundary can drive the pin LOW before consuming the
    // claim. Missing/hung GPIO tooling therefore cannot strand a paid order.
    const preflightDriver = gpio('low', false);

    // 3. The claim layer re-validates the raw ToolResult and immutable order,
    // then creates an exclusive O_EXCL claim. A second run cannot pass.
    let claim;
    try {
      claim = claimOrder({ reference, ordersDir, rawWatchResult, driverId: 'proofkiosk-actuator-1' });
    } catch (error) {
      journal(root, { event: 'refused', reference, reason: error.message });
      throw new Error(`claim refused — NOT firing: ${error.message}`);
    }

    process.stdout.write(`claim ${claim.claim_id} granted for ${reference}\n`);
    process.stdout.write(`firing GPIO${PIN_BCM} for ${pulse}ms\n`);
    journal(root, { event: 'actuating', reference, claim_id: claim.claim_id, pin: PIN_BCM, pulse_ms: pulse });

    let pulseError;
    let driver = preflightDriver;
    try {
      driver = gpio('high', false);
      sleep(pulse);
    } catch (error) {
      pulseError = error;
    } finally {
      try {
        gpio('low', false);
      } catch (error) {
        pulseError ??= error;
      }
    }

    if (pulseError) {
      journal(root, { event: 'fault', reference, claim_id: claim.claim_id, reason: pulseError.message });
      throw new Error(`GPIO fault after claim; inspect hardware and journal — NOT retrying automatically: ${pulseError.message}`);
    }

    // A GPIO pulse is not proof that an item arrived. Only a delivery sensor
    // may promote this state to `delivered` in a future recovery driver.
    journal(root, { event: 'pulse_completed', reference, claim_id: claim.claim_id, pin: PIN_BCM, pulse_ms: pulse, driver });
    process.stdout.write(`pulse_completed: ${reference} (${driver}); delivery sensor not present\n`);
  }
} catch (error) {
  process.stderr.write(`actuator: ${error.message}\n`);
  process.exitCode = 1;
} finally {
  try { if (lockFd !== undefined) fs.closeSync(lockFd); } catch { /* best effort */ }
  try { fs.unlinkSync(lockFile); } catch { /* a stale lock fails closed next run */ }
}
