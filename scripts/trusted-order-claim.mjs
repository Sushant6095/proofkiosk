#!/usr/bin/env node

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

const MAX_BYTES = 65_536;
const MAX_PAYMENT_WINDOW_S = 86_400;
const PAYMENT_CLOCK_SKEW_MS = 30_000;
const BASE58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const WATCH_KEYS = new Set([
  'v', 'success', 'status', 'payer', 'signature', 'slot', 'reference', 'item_id',
  'amount', 'recipient', 'mint', 'token_decimals', 'payment_window_s',
  'payment_block_time_s',
  'payment_verified', 'actuation_authorized', 'requires_atomic_claim', 'message',
]);
const ORDER_KEYS = new Set([
  'v', 'status', 'reference', 'item_id', 'amount', 'recipient', 'mint',
  'token_decimals', 'payment_window_s', 'url', 'created_at_ms', 'expires_at_ms',
  'raw_output_sha256',
]);

function fail(message) {
  throw new Error(message);
}

function decodeBase58Length(text) {
  if (typeof text !== 'string' || !text) return -1;
  let value = 0n;
  for (const char of text) {
    const digit = BASE58.indexOf(char);
    if (digit < 0) return -1;
    value = value * 58n + BigInt(digit);
  }
  let bytes = 0;
  for (let copy = value; copy > 0n; copy >>= 8n) bytes += 1;
  let zeroes = 0;
  while (zeroes < text.length && text[zeroes] === '1') zeroes += 1;
  return bytes + zeroes;
}

function parseExactJson(raw, label) {
  if (Buffer.byteLength(raw, 'utf8') > MAX_BYTES) fail(`${label} exceeds 64 KiB`);
  try {
    return JSON.parse(raw);
  } catch {
    fail(`${label} must be exact JSON from a host-direct result, never model prose`);
  }
}

function unwrapToolResult(parsed) {
  if (parsed && typeof parsed === 'object' && typeof parsed.output === 'string') {
    if (!Object.keys(parsed).every((key) => ['success', 'output', 'error'].includes(key))) {
      fail('unrecognized ToolResult wrapper fields');
    }
    if (parsed.success !== true || parsed.error != null) fail('watch ToolResult was not successful');
    return parseExactJson(parsed.output, 'watch ToolResult output');
  }
  return parsed;
}

export function validateClaim(rawWatchResult, order) {
  const watch = unwrapToolResult(parseExactJson(rawWatchResult, 'watch result'));
  if (!watch || typeof watch !== 'object' || Array.isArray(watch)
      || Object.keys(watch).length !== WATCH_KEYS.size
      || !Object.keys(watch).every((key) => WATCH_KEYS.has(key))) {
    fail('watch machine output has an invalid shape');
  }
  if (watch.v !== 1 || watch.success !== true || watch.status !== 'paid'
      || watch.payment_verified !== true || watch.actuation_authorized !== false
      || watch.requires_atomic_claim !== true) {
    fail('watch result is not a claim-eligible verified payment');
  }
  if (!Number.isSafeInteger(watch.slot) || watch.slot <= 0) fail('watch slot is invalid');
  if (decodeBase58Length(watch.signature) !== 64) fail('watch signature is not a Solana signature');
  if (!order || typeof order !== 'object' || Array.isArray(order)
      || Object.keys(order).length !== ORDER_KEYS.size
      || !Object.keys(order).every((key) => ORDER_KEYS.has(key))
      || order.v !== 1 || order.status !== 'created') {
    fail('persisted order is not claimable');
  }
  for (const [value, label] of [
    [watch.payer, 'watch payer'],
    [watch.recipient, 'watch recipient'],
    [watch.mint, 'watch mint'],
    [order.reference, 'order reference'],
    [order.recipient, 'order recipient'],
    [order.mint, 'order mint'],
  ]) {
    if (decodeBase58Length(value) !== 32) fail(`${label} is not a Solana pubkey`);
  }
  if (!/^(?:0|[1-9]\d*)(?:\.\d*[1-9])?$/u.test(order.amount)
      || !/^(?:0|[1-9]\d*)(?:\.\d*[1-9])?$/u.test(watch.amount)
      || order.amount === '0' || watch.amount === '0') {
    fail('order/watch amount is not a positive canonical decimal');
  }
  if (!/^[A-Za-z0-9_-]{1,64}$/u.test(order.item_id)
      || watch.item_id !== order.item_id || typeof watch.message !== 'string') {
    fail('order/watch item or message is invalid');
  }
  if (!Number.isInteger(order.token_decimals) || order.token_decimals < 0
      || order.token_decimals > 18 || !Number.isInteger(watch.token_decimals)
      || watch.token_decimals < 0 || watch.token_decimals > 18) {
    fail('order/watch token_decimals is invalid');
  }
  if (!Number.isSafeInteger(order.payment_window_s) || order.payment_window_s < 1
      || order.payment_window_s > MAX_PAYMENT_WINDOW_S
      || !Number.isSafeInteger(watch.payment_window_s) || watch.payment_window_s < 1
      || watch.payment_window_s > MAX_PAYMENT_WINDOW_S) {
    fail('order/watch payment_window_s is invalid');
  }
  if (!Number.isSafeInteger(order.created_at_ms) || order.created_at_ms < 0
      || !Number.isSafeInteger(order.expires_at_ms)
      || order.expires_at_ms !== order.created_at_ms + (order.payment_window_s * 1000)) {
    fail('persisted order timestamps are invalid');
  }
  if (!Number.isSafeInteger(watch.payment_block_time_s) || watch.payment_block_time_s < 0
      || watch.payment_block_time_s > Math.floor(Number.MAX_SAFE_INTEGER / 1000)) {
    fail('watch payment block time is invalid');
  }
  if (typeof order.url !== 'string' || !/^[a-f0-9]{64}$/u.test(order.raw_output_sha256)) {
    fail('persisted order provenance is invalid');
  }
  if (watch.reference !== order.reference || watch.item_id !== order.item_id) {
    fail('watch result does not identify this persisted order');
  }
  if (watch.amount !== order.amount || watch.recipient !== order.recipient
      || watch.mint !== order.mint || watch.token_decimals !== order.token_decimals
      || watch.payment_window_s !== order.payment_window_s) {
    fail('watch result does not match the persisted order economics and policy');
  }
  const paymentAtMs = watch.payment_block_time_s * 1000;
  if (paymentAtMs + PAYMENT_CLOCK_SKEW_MS < order.created_at_ms) {
    fail('verified payment predates the persisted quote');
  }
  if (paymentAtMs > order.expires_at_ms) {
    fail('verified payment landed after the persisted quote expired');
  }
  return watch;
}

function readRegular(filename, label) {
  const stats = fs.lstatSync(filename);
  if (!stats.isFile() || stats.isSymbolicLink() || stats.size > MAX_BYTES) {
    fail(`${label} must be a regular file no larger than 64 KiB`);
  }
  return fs.readFileSync(filename, 'utf8');
}

function fsyncDirectory(directory) {
  const fd = fs.openSync(directory, 'r');
  try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
}

export function inspectOrder({ reference, ordersDir, rawWatchResult }) {
  if (decodeBase58Length(reference) !== 32) fail('reference is not a Solana pubkey');
  const directoryStats = fs.lstatSync(ordersDir);
  if (!directoryStats.isDirectory() || directoryStats.isSymbolicLink()) fail('orders directory is not trusted');
  const orderFile = path.join(ordersDir, `${reference}.json`);
  const orderRaw = readRegular(orderFile, 'persisted order');
  const order = parseExactJson(orderRaw, 'persisted order');
  if (order.reference !== reference) fail('persisted order filename/reference mismatch');
  const watch = validateClaim(rawWatchResult, order);
  return { order, orderRaw, watch };
}

export function claimOrder({ reference, ordersDir, rawWatchResult, driverId, nowMs = Date.now() }) {
  if (!/^[A-Za-z0-9_-]{1,64}$/u.test(driverId)) fail('driver-id is invalid');
  const { order, orderRaw, watch } = inspectOrder({ reference, ordersDir, rawWatchResult });
  if (!Number.isSafeInteger(nowMs) || nowMs < 0) fail('claim time is invalid');
  const claim = {
    v: 1,
    status: 'claimed',
    claim_id: crypto.randomBytes(16).toString('hex'),
    driver_id: driverId,
    reference,
    item_id: order.item_id,
    amount: order.amount,
    recipient: order.recipient,
    mint: order.mint,
    token_decimals: order.token_decimals,
    payment_signature: watch.signature,
    payment_slot: watch.slot,
    payment_block_time_s: watch.payment_block_time_s,
    claimed_at_ms: nowMs,
    order_sha256: crypto.createHash('sha256').update(orderRaw).digest('hex'),
    watch_result_sha256: crypto.createHash('sha256').update(rawWatchResult).digest('hex'),
  };
  const claimFile = path.join(ordersDir, `${reference}.claim.json`);
  let fd;
  try {
    fd = fs.openSync(claimFile, 'wx', 0o600);
  } catch (error) {
    if (error?.code === 'EEXIST') fail('order was already claimed; actuator must not fire again');
    throw error;
  }
  try {
    fs.writeFileSync(fd, `${JSON.stringify(claim, null, 2)}\n`, 'utf8');
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  fsyncDirectory(ordersDir);
  return { ...claim, claim_file: claimFile };
}

function parseArgs(argv) {
  const result = { ordersDir: '.proofkiosk/orders' };
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!value || !['--reference', '--orders-dir', '--watch-result', '--driver-id'].includes(key)) {
      fail('usage: trusted-order-claim.mjs --reference PUBKEY --watch-result RAW.json --driver-id ID [--orders-dir DIR]');
    }
    result[key.slice(2).replace(/-([a-z])/gu, (_, letter) => letter.toUpperCase())] = value;
  }
  if (!result.reference || !result.watchResult || !result.driverId) fail('reference, watch-result, and driver-id are required');
  return result;
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const rawWatchResult = readRegular(options.watchResult, 'watch result');
  const claim = claimOrder({ ...options, rawWatchResult });
  process.stdout.write(`${JSON.stringify({ success: true, ...claim })}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`trusted claim rejected: ${error.message}\n`);
    process.exitCode = 1;
  }
}
