#!/usr/bin/env node
/**
 * ProofKiosk customer display.
 *
 * Serves the kiosk screen and a /state endpoint derived ONLY from the durable
 * files the trusted boundary already writes:
 *
 *   .proofkiosk/orders/<reference>.json         the order (charge handoff)
 *   .proofkiosk/orders/<reference>.claim.json   the exclusive claim
 *   .proofkiosk/actuator-journal.jsonl          actuating / delivered / refused
 *
 * It never talks to an RPC node and never decides anything. If a stage is not
 * on disk, the screen does not show it — the display cannot invent progress.
 *
 *   node scripts/kiosk-display.mjs [--port 8080] [--root .]
 */
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

const arg = (flag, fallback) => {
  const i = process.argv.indexOf(flag);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
};

const root = path.resolve(arg('--root', path.join(HERE, '..')));
const port = Number(arg('--port', '8080'));
const ordersDir = path.join(root, '.proofkiosk/orders');
const journalFile = path.join(root, '.proofkiosk/actuator-journal.jsonl');
const configFile = path.resolve(arg('--config', path.join(root, '.devnet/zeroclaw.toml')));

const readJson = (file) => {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return null; }
};

/**
 * The menu is read from the SAME operator config the trusted handoff validates
 * against — not from a separate list. A price shown on the screen and the price
 * the verifier gates on therefore cannot drift apart.
 *
 * Deliberately a narrow parse of the `kiosk-charge` entry rather than a TOML
 * dependency: this runs on a Pi and reads one known file.
 */
function readMenu() {
  let toml;
  try { toml = fs.readFileSync(configFile, 'utf8'); } catch { return null; }

  const blocks = toml.split(/^\s*\[\[plugins\.entries\]\]\s*$/m);
  const charge = blocks.find((b) => /name\s*=\s*"kiosk-charge"/.test(b));
  if (!charge) return null;

  const field = (key) => charge.match(new RegExp(`^\\s*${key}\\s*=\\s*"([^"]*)"`, 'm'))?.[1] ?? null;

  const raw = field('price_list');
  const items = (raw ?? '')
    .split(',')
    .map((pair) => pair.trim())
    .filter(Boolean)
    .map((pair) => {
      const [id, price] = pair.split(':').map((s) => s.trim());
      return id && price ? { item_id: id, price } : null;
    })
    .filter(Boolean);

  return {
    label: field('label'),
    merchant: field('merchant_address'),
    max_amount: field('max_amount_usdc'),
    items,
  };
}

/** Newest order by creation time. `.claim.json` siblings are not orders. */
function newestOrder() {
  let entries;
  try { entries = fs.readdirSync(ordersDir); } catch { return null; }
  const orders = entries
    .filter((f) => f.endsWith('.json') && !f.endsWith('.claim.json'))
    .map((f) => readJson(path.join(ordersDir, f)))
    .filter((o) => o && o.reference);
  if (!orders.length) return null;
  return orders.sort((a, b) => (b.created_at_ms ?? 0) - (a.created_at_ms ?? 0))[0];
}

function journalFor(reference) {
  let lines;
  try { lines = fs.readFileSync(journalFile, 'utf8').split('\n'); } catch { return []; }
  return lines
    .filter(Boolean)
    .map((l) => { try { return JSON.parse(l); } catch { return null; } })
    .filter((e) => e && e.reference === reference);
}

/**
 * Derive the stage from evidence on disk. Precedence runs backwards from the
 * strongest artifact, so a later refusal never erases a delivery that happened.
 */
export function stageOf(order, claim, events, nowMs = Date.now()) {
  if (events.some((e) => e.event === 'delivered')) return 'delivered';
  if (events.some((e) => e.event === 'actuating')) return 'dispensing';
  if (claim) return 'claimed';
  if (order.expires_at_ms && nowMs > order.expires_at_ms) return 'expired';
  return 'awaiting';
}

/** Inline SVG QR via qrencode. Missing binary is reported, never faked. */
let qrCache = { url: null, svg: null };
function qrSvg(url) {
  if (qrCache.url === url) return qrCache.svg;
  let svg = null;
  try {
    svg = execFileSync('qrencode', ['-t', 'SVG', '-m', '0', '-s', '6', '-l', 'M', '-o', '-', url],
      { encoding: 'utf8', maxBuffer: 4 << 20 });
  } catch {
    svg = null; // surfaced to the page as qr_available:false
  }
  qrCache = { url, svg };
  return svg;
}

function buildState() {
  const menu = readMenu();
  const order = newestOrder();
  // Idle is not an error state: with no order the kiosk shows its menu, which
  // is what a customer walks up to.
  if (!order) return { ok: false, reason: 'no order on disk yet', menu, now_ms: Date.now() };

  const claim = readJson(path.join(ordersDir, `${order.reference}.claim.json`));
  const events = journalFor(order.reference);
  const stage = stageOf(order, claim, events);
  const refusals = events.filter((e) => e.event === 'refused');
  const svg = qrSvg(order.url);

  return {
    ok: true,
    stage,
    menu,
    order: {
      reference: order.reference,
      item_id: order.item_id,
      amount: order.amount,
      recipient: order.recipient,
      mint: order.mint,
      token_decimals: order.token_decimals,
      expires_at_ms: order.expires_at_ms,
      url: order.url,
    },
    claim: claim && {
      claim_id: claim.claim_id,
      driver_id: claim.driver_id,
      payment_signature: claim.payment_signature,
      payment_slot: claim.payment_slot,
      claimed_at_ms: claim.claimed_at_ms,
    },
    delivery: events.find((e) => e.event === 'delivered') ?? null,
    last_refusal: refusals.length ? refusals[refusals.length - 1] : null,
    // The QR is served separately from /qr and cached by the browser — inlining
    // ~140 KB of SVG into a 1 Hz poll would be the whole bandwidth budget.
    qr_available: Boolean(svg),
    now_ms: Date.now(),
  };
}

const send = (res, code, type, body) => {
  res.writeHead(code, { 'content-type': type, 'cache-control': 'no-store' });
  res.end(body);
};

// Importable for tests: only serve when run directly.
const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) http.createServer((req, res) => {
  const url = (req.url || '/').split('?')[0];
  if (url === '/state') return send(res, 200, 'application/json', JSON.stringify(buildState()));
  if (url === '/qr') {
    const order = newestOrder();
    const svg = order && qrSvg(order.url);
    if (!svg) return send(res, 503, 'text/plain', 'qrencode unavailable');
    // Cacheable on purpose: the ?ref query changes when the order does, so a
    // long max-age is safe and keeps the 1 Hz poll from refetching 140 KB.
    res.writeHead(200, { 'content-type': 'image/svg+xml', 'cache-control': 'public, max-age=86400' });
    return res.end(svg);
  }
  if (url === '/' || url === '/index.html') {
    try {
      return send(res, 200, 'text/html; charset=utf-8',
        fs.readFileSync(path.join(HERE, 'kiosk-display.html')));
    } catch (e) {
      return send(res, 500, 'text/plain', `cannot read kiosk-display.html: ${e.message}`);
    }
  }
  send(res, 404, 'text/plain', 'not found');
}).listen(port, () => {
  process.stdout.write(`ProofKiosk display on http://localhost:${port}  (root ${root})\n`);
  if (!newestOrder()) process.stdout.write('  no order yet — run a charge to populate the screen\n');
  try { execFileSync('qrencode', ['--version'], { stdio: 'ignore' }); }
  catch { process.stdout.write('  WARNING: qrencode missing — sudo apt install -y qrencode\n'); }
});
