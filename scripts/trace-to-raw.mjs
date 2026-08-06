#!/usr/bin/env node
/**
 * Rebuild a raw WIT `ToolResult` from the host's runtime trace.
 *
 *   node scripts/trace-to-raw.mjs kiosk_charge  .devnet/charge.json
 *   node scripts/trace-to-raw.mjs kiosk_watch   .devnet/watch.json
 *
 * The trace records the tool's own output as the host saw it, but stores only
 * the inner JSON string — `trusted-charge-handoff.mjs` and the claim layer both
 * want the `{success, output, error}` envelope, so reconstruct it.
 *
 * This is a convenience for driving a live agent demo, NOT a new trust anchor.
 * Nothing here is trusted: the handoff re-derives the Solana Pay URI and checks
 * every economic field against operator config, so a doctored trace is refused
 * downstream exactly like any other bad input.
 */
import fs from 'node:fs';
import path from 'node:path';

const [tool, outFile] = process.argv.slice(2);
if (!tool || !outFile) {
  process.stderr.write('usage: trace-to-raw.mjs <tool_name> <output.json> [--trace FILE]\n');
  process.exit(2);
}

const flag = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
};

const configDir = process.env.ZC_CONFIG_DIR
  || path.join(process.env.PROOFKIOSK_ROOT || process.cwd(), '.devnet/zeroclaw-config');
const traceFile = flag('--trace', path.join(configDir, 'data/state/runtime-trace.jsonl'));

let lines;
try {
  lines = fs.readFileSync(traceFile, 'utf8').split('\n').filter(Boolean);
} catch (e) {
  process.stderr.write(`cannot read trace ${traceFile}: ${e.message}\n`);
  process.exit(1);
}

// Newest matching tool_call_result wins.
let picked = null;
for (const line of lines) {
  let event;
  try { event = JSON.parse(line); } catch { continue; }
  const a = event.attributes;
  if (a?.tool === tool && typeof a.output === 'string') picked = a;
}

if (!picked) {
  process.stderr.write(`no trace entry found for tool '${tool}' in ${traceFile}\n`);
  process.exit(1);
}

const wrapper = {
  error: picked.error_reason ?? null,
  output: picked.output,
  success: picked.error_reason == null,
};

fs.writeFileSync(outFile, `${JSON.stringify(wrapper)}\n`, { mode: 0o600 });

const inner = JSON.parse(picked.output);
process.stdout.write(
  `wrote ${outFile}\n` +
  `  tool      ${tool}\n` +
  `  status    ${inner.status ?? '—'}\n` +
  `  reference ${inner.reference ?? '—'}\n`,
);
