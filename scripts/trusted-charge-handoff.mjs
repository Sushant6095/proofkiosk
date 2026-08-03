#!/usr/bin/env node

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

const MAX_INPUT_BYTES = 65_536;
const MAX_U64 = (1n << 64n) - 1n;
const DEFAULT_PAYMENT_WINDOW_S = 900;
const MAX_PAYMENT_WINDOW_S = 86_400;
const BASE58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const OUTPUT_KEYS = new Set([
  'v', 'success', 'status', 'actuation_eligible', 'reference', 'item_id',
  'amount', 'recipient', 'mint', 'created_at_ms', 'url', 'message',
]);
const URL_KEYS = new Set(['amount', 'spl-token', 'reference', 'label', 'message', 'memo']);

function fail(message) {
  throw new Error(message);
}

function parseTomlScalar(raw) {
  const value = raw.trim();
  if (value.startsWith('"')) {
    try {
      return JSON.parse(value);
    } catch {
      fail('config contains an invalid quoted string');
    }
  }
  if (value.startsWith("'") && value.endsWith("'")) return value.slice(1, -1);
  return value;
}

function withoutComment(line) {
  let quoted = false;
  let escaped = false;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if (escaped) {
      escaped = false;
    } else if (char === '\\' && quoted) {
      escaped = true;
    } else if (char === '"') {
      quoted = !quoted;
    } else if (char === '#' && !quoted) {
      return line.slice(0, i);
    }
  }
  return line;
}

export function parsePluginConfig(toml) {
  if (Buffer.byteLength(toml, 'utf8') > MAX_INPUT_BYTES) fail('config exceeds 64 KiB');
  const plugins = new Map();
  let row = null;
  let inConfig = false;
  for (const sourceLine of toml.split(/\r?\n/u)) {
    const line = withoutComment(sourceLine).trim();
    if (!line) continue;
    if (line === '[[plugins.entries]]') {
      row = { name: null, config: Object.create(null) };
      inConfig = false;
      continue;
    }
    if (line.startsWith('[')) {
      inConfig = line === '[plugins.entries.config]' && row !== null;
      continue;
    }
    const match = line.match(/^([A-Za-z0-9_-]+)\s*=\s*(.+)$/u);
    if (!match || !row) continue;
    const [, key, raw] = match;
    const value = parseTomlScalar(raw);
    if (!inConfig && key === 'name') {
      if (typeof value !== 'string' || !value) fail('plugin row has an invalid name');
      if (plugins.has(value)) fail(`config contains duplicate plugin row ${value}`);
      row.name = value;
      plugins.set(value, row.config);
    } else if (inConfig) {
      if (!row.name) fail('plugin config appears before its name');
      if (Object.hasOwn(row.config, key)) fail(`duplicate ${row.name}.${key}`);
      if (typeof value !== 'string') fail(`${row.name}.${key} must be a string`);
      if (value.startsWith('enc2:')) fail(`cannot validate encrypted ${row.name}.${key}`);
      row.config[key] = value;
    }
  }
  return plugins;
}

function decodeBase58Length(text) {
  if (typeof text !== 'string' || text.length === 0) return -1;
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

function requirePubkey(value, name) {
  if (decodeBase58Length(value) !== 32) fail(`${name} is not a 32-byte base58 pubkey`);
}

function decimal(value, decimals, name) {
  if (typeof value !== 'string' || !/^\d+(?:\.\d+)?$/u.test(value)) {
    fail(`${name} is not a plain decimal string`);
  }
  const [whole, fraction = ''] = value.split('.');
  if (fraction.length > decimals) fail(`${name} exceeds token_decimals`);
  const normalizedWhole = whole.replace(/^0+(?=\d)/u, '');
  const normalizedFraction = fraction.replace(/0+$/u, '');
  const normalized = normalizedFraction ? `${normalizedWhole}.${normalizedFraction}` : normalizedWhole;
  const units = BigInt(`${normalizedWhole}${fraction.padEnd(decimals, '0')}`);
  if (units <= 0n) {
    fail(`${name} must be greater than zero`);
  }
  if (units > MAX_U64) fail(`${name} exceeds Solana's u64 token amount`);
  return normalized;
}

function priceMap(raw, decimals) {
  const prices = new Map();
  for (const entry of raw.split(',').map((value) => value.trim()).filter(Boolean)) {
    const separator = entry.indexOf(':');
    if (separator <= 0) fail(`invalid price_list entry ${entry}`);
    const item = entry.slice(0, separator).trim();
    if (!/^[A-Za-z0-9_-]{1,64}$/u.test(item)) fail(`invalid item id ${item}`);
    if (prices.has(item)) fail(`duplicate price item ${item}`);
    prices.set(item, decimal(entry.slice(separator + 1).trim(), decimals, `price ${item}`));
  }
  return prices;
}

function directMachineOutput(raw) {
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    fail('input must be one raw JSON ToolResult or machine-output object, never model prose');
  }
  if (parsed && typeof parsed === 'object' && typeof parsed.output === 'string') {
    const wrapperKeys = Object.keys(parsed);
    if (!wrapperKeys.every((key) => ['success', 'output', 'error'].includes(key))) {
      fail('unrecognized ToolResult wrapper fields');
    }
    if (parsed.success !== true || parsed.error != null) fail('plugin ToolResult was not successful');
    try {
      parsed = JSON.parse(parsed.output);
    } catch {
      fail('ToolResult output is not the exact machine JSON');
    }
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) fail('machine output must be an object');
  return parsed;
}

export function validateHandoff(rawOutput, configToml) {
  if (Buffer.byteLength(rawOutput, 'utf8') > MAX_INPUT_BYTES) fail('charge output exceeds 64 KiB');
  const machine = directMachineOutput(rawOutput);
  if (!Object.keys(machine).every((key) => OUTPUT_KEYS.has(key))) fail('machine output has unknown fields');
  if (machine.v !== 1 || machine.success !== true || machine.status !== 'created') {
    fail('machine output is not a successful v1 created charge');
  }
  if (machine.actuation_eligible !== true || typeof machine.item_id !== 'string') {
    fail('trusted QR handoff accepts catalog-item charges only');
  }
  if (typeof machine.url !== 'string' || /[\u0000-\u0020\u007f]/u.test(machine.url)) {
    fail('charge URL contains whitespace or control characters');
  }
  if (!Number.isSafeInteger(machine.created_at_ms) || machine.created_at_ms < 0) {
    fail('created_at_ms is invalid');
  }
  requirePubkey(machine.reference, 'reference');
  requirePubkey(machine.recipient, 'recipient');
  requirePubkey(machine.mint, 'mint');

  const plugins = parsePluginConfig(configToml);
  const charge = plugins.get('kiosk-charge');
  const watch = plugins.get('kiosk-watch');
  if (!charge || !watch) fail('config must contain kiosk-charge and kiosk-watch');
  const required = (section, key) => {
    const value = section[key];
    if (!value) fail(`config is missing ${key}`);
    return value;
  };
  const recipient = required(charge, 'merchant_address');
  const mint = required(charge, 'usdc_mint');
  const decimalsText = required(charge, 'token_decimals');
  if (!/^\d+$/u.test(decimalsText) || Number(decimalsText) > 18) fail('token_decimals must be 0..18');
  const decimals = Number(decimalsText);
  if (watch.merchant_address !== recipient || watch.usdc_mint !== mint || watch.token_decimals !== decimalsText) {
    fail('charge/watch recipient, mint, or token_decimals differ');
  }
  const paymentWindowText = watch.payment_window_s ?? String(DEFAULT_PAYMENT_WINDOW_S);
  if (!/^\d+$/u.test(paymentWindowText)) fail('payment_window_s must be an integer');
  const paymentWindowS = Number(paymentWindowText);
  if (!Number.isSafeInteger(paymentWindowS) || paymentWindowS < 1
      || paymentWindowS > MAX_PAYMENT_WINDOW_S) {
    fail(`payment_window_s must be 1..${MAX_PAYMENT_WINDOW_S}`);
  }
  const expiresAtMs = machine.created_at_ms + (paymentWindowS * 1000);
  if (!Number.isSafeInteger(expiresAtMs)) fail('quote expiry exceeds the safe timestamp range');
  const expected = priceMap(required(charge, 'price_list'), decimals).get(machine.item_id);
  if (!expected) fail('item_id is not in the operator price_list');
  if (priceMap(required(watch, 'price_list'), decimals).get(machine.item_id) !== expected) {
    fail('charge/watch price_list differs for this item');
  }
  if (machine.recipient !== recipient || machine.mint !== mint) fail('machine recipient or mint differs from config');
  if (decimal(machine.amount, decimals, 'machine amount') !== expected) fail('machine amount differs from catalog price');

  let uri;
  try {
    uri = new URL(machine.url);
  } catch {
    fail('charge URL is invalid');
  }
  if (uri.protocol !== 'solana:' || uri.pathname !== recipient || uri.hash) fail('charge URL target is invalid');
  const counts = new Map();
  for (const key of uri.searchParams.keys()) {
    if (!URL_KEYS.has(key)) fail(`charge URL has unknown parameter ${key}`);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  for (const key of ['amount', 'spl-token', 'reference', 'memo']) {
    if (counts.get(key) !== 1) fail(`charge URL must contain exactly one ${key}`);
  }
  if (decimal(uri.searchParams.get('amount'), decimals, 'URL amount') !== expected) fail('URL amount differs from catalog');
  if (uri.searchParams.get('spl-token') !== mint) fail('URL mint differs from config');
  if (uri.searchParams.get('reference') !== machine.reference) fail('URL reference differs from machine output');
  let memo;
  try {
    memo = JSON.parse(uri.searchParams.get('memo'));
  } catch {
    fail('URL memo is not JSON');
  }
  const expectedMemo = { v: 1, tag: 'PKPAY1', ref: machine.reference, item: machine.item_id };
  if (!memo || typeof memo !== 'object' || Array.isArray(memo)
      || Object.keys(memo).sort().join(',') !== Object.keys(expectedMemo).sort().join(',')
      || memo.v !== expectedMemo.v || memo.tag !== expectedMemo.tag
      || memo.ref !== expectedMemo.ref || memo.item !== expectedMemo.item) {
    fail('URL memo does not exactly bind reference and item');
  }
  return {
    ...machine,
    amount: expected,
    token_decimals: decimals,
    payment_window_s: paymentWindowS,
    expires_at_ms: expiresAtMs,
    url: uri.href,
  };
}

function parseArgs(argv) {
  const options = { ordersDir: '.proofkiosk/orders', urlOnly: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--url-only') options.urlOnly = true;
    else if (['--input', '--config', '--orders-dir'].includes(arg)) {
      const value = argv[index + 1];
      if (!value) fail(`${arg} requires a value`);
      options[arg.slice(2).replace(/-([a-z])/gu, (_, letter) => letter.toUpperCase())] = value;
      index += 1;
    } else fail(`unknown argument ${arg}`);
  }
  if (!options.input || !options.config) fail('usage: trusted-charge-handoff.mjs --input RAW.json --config config.toml [--orders-dir DIR] [--url-only]');
  return options;
}

function readBounded(filename) {
  const stats = fs.statSync(filename);
  if (!stats.isFile() || stats.size > MAX_INPUT_BYTES) fail(`${filename} must be a regular file no larger than 64 KiB`);
  return fs.readFileSync(filename, 'utf8');
}

function persistOrder(order, ordersDir, rawOutput) {
  fs.mkdirSync(ordersDir, { recursive: true, mode: 0o700 });
  const directoryStats = fs.lstatSync(ordersDir);
  if (!directoryStats.isDirectory() || directoryStats.isSymbolicLink()) fail('orders directory is not trusted');
  const filename = path.join(ordersDir, `${order.reference}.json`);
  const record = {
    v: 1,
    status: 'created',
    reference: order.reference,
    item_id: order.item_id,
    amount: order.amount,
    recipient: order.recipient,
    mint: order.mint,
    token_decimals: order.token_decimals,
    payment_window_s: order.payment_window_s,
    url: order.url,
    created_at_ms: order.created_at_ms,
    expires_at_ms: order.expires_at_ms,
    raw_output_sha256: crypto.createHash('sha256').update(rawOutput).digest('hex'),
  };
  const serialized = `${JSON.stringify(record, null, 2)}\n`;
  try {
    const fd = fs.openSync(filename, 'wx', 0o600);
    try {
      fs.writeFileSync(fd, serialized, 'utf8');
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    const directoryFd = fs.openSync(ordersDir, 'r');
    try { fs.fsyncSync(directoryFd); } finally { fs.closeSync(directoryFd); }
  } catch (error) {
    if (error?.code !== 'EEXIST' || fs.readFileSync(filename, 'utf8') !== serialized) {
      fail(`order ${order.reference} already exists with different trusted state`);
    }
  }
  return filename;
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const rawOutput = readBounded(options.input);
  const order = validateHandoff(rawOutput, readBounded(options.config));
  const orderFile = persistOrder(order, options.ordersDir, rawOutput);
  if (options.urlOnly) process.stdout.write(`${order.url}\n`);
  else process.stdout.write(`${JSON.stringify({ success: true, order_file: orderFile, ...order })}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`trusted handoff rejected: ${error.message}\n`);
    process.exitCode = 1;
  }
}
