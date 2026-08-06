import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from 'playwright';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDir, '..');
const researchDir = path.join(root, 'research');
const outputDir = path.join(root, 'artifacts');
const htmlPath = path.join(outputDir, 'ProofKiosk-System-Handbook.html');
const pdfPath = path.join(outputDir, 'ProofKiosk-System-Handbook.pdf');

fs.mkdirSync(outputDir, { recursive: true });

const read = (name) => fs.readFileSync(path.join(researchDir, name), 'utf8');

const ecosystem = read('ecosystem-research.md');
const atlas = read('repo-atlas.md');
const competition = fs.readFileSync(path.join(root, 'docs', 'FINAL-READINESS-AUDIT.md'), 'utf8');
const actualCommit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
const trackedDirty = execFileSync(
  'git',
  ['status', '--porcelain', '--untracked-files=no'],
  { cwd: root, encoding: 'utf8' },
).trim().length > 0;
const snapshotLabel = `${actualCommit.slice(0, 7)}${trackedDirty ? '+working-tree' : ''}`;
const trackedFileCount = Number(
  execFileSync('git', ['ls-files'], { cwd: root, encoding: 'utf8' })
    .trim()
    .split('\n')
    .filter(Boolean).length,
);

function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function slugify(value) {
  return value
    .toLowerCase()
    .replace(/<[^>]+>/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80);
}

function renderInline(source) {
  const linkTokens = [];
  const codeTokens = [];
  let text = escapeHtml(source);

  text = text.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, label, url) => {
    const token = `@@LINK${linkTokens.length}@@`;
    const labelHtml = label.replace(/`([^`]+)`/g, '<code>$1</code>');
    linkTokens.push(`<a href="${url}" target="_blank" rel="noreferrer">${labelHtml}</a>`);
    return token;
  });

  text = text.replace(/`([^`]+)`/g, (_, code) => {
    const token = `@@INLINECODE${codeTokens.length}@@`;
    codeTokens.push(`<code>${code}</code>`);
    return token;
  });

  text = text
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/__([^_]+)__/g, '<strong>$1</strong>')
    .replace(/(?<!\*)\*([^*]+)\*(?!\*)/g, '<em>$1</em>');

  codeTokens.forEach((token, index) => {
    text = text.replaceAll(`@@INLINECODE${index}@@`, token);
  });
  linkTokens.forEach((token, index) => {
    text = text.replaceAll(`@@LINK${index}@@`, token);
  });

  return text;
}

function isTableSeparator(line) {
  return /^\s*\|?\s*:?-{3,}/.test(line) && line.includes('|');
}

function splitTableRow(line) {
  return line
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split('|')
    .map((cell) => cell.trim());
}

function markdownToHtml(markdown, prefix) {
  const lines = markdown.replaceAll('\r', '').split('\n');
  const html = [];
  let index = 0;
  let codeFence = false;
  let codeLanguage = '';
  let codeLines = [];
  let listType = null;
  let listStart = 1;
  let headingCounter = 0;

  const closeList = () => {
    if (listType) {
      html.push(`</${listType}>`);
      listType = null;
    }
  };

  while (index < lines.length) {
    const line = lines[index];
    const trimmed = line.trim();

    if (trimmed.startsWith('```')) {
      closeList();
      if (!codeFence) {
        codeFence = true;
        codeLanguage = trimmed.slice(3).trim();
        codeLines = [];
      } else {
        html.push(`<pre data-language="${escapeHtml(codeLanguage || 'text')}"><code>${escapeHtml(codeLines.join('\n'))}</code></pre>`);
        codeFence = false;
        codeLanguage = '';
        codeLines = [];
      }
      index += 1;
      continue;
    }

    if (codeFence) {
      codeLines.push(line);
      index += 1;
      continue;
    }

    if (!trimmed) {
      closeList();
      index += 1;
      continue;
    }

    if (/^( {4}|\t)/.test(line)) {
      closeList();
      const indented = [];
      while (index < lines.length) {
        const candidate = lines[index];
        if (/^( {4}|\t)/.test(candidate)) {
          indented.push(candidate.replace(/^( {4}|\t)/, ''));
          index += 1;
          continue;
        }
        if (candidate.trim() === '' && index + 1 < lines.length && /^( {4}|\t)/.test(lines[index + 1])) {
          indented.push('');
          index += 1;
          continue;
        }
        break;
      }
      html.push(`<pre data-language="text"><code>${escapeHtml(indented.join('\n'))}</code></pre>`);
      continue;
    }

    if (/^---+$/.test(trimmed)) {
      closeList();
      html.push('<hr>');
      index += 1;
      continue;
    }

    const heading = trimmed.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      closeList();
      const level = Math.min(6, heading[1].length);
      const title = renderInline(heading[2]);
      const id = `${prefix}-${slugify(heading[2]) || `section-${headingCounter++}`}`;
      html.push(`<h${level} id="${id}">${title}</h${level}>`);
      index += 1;
      continue;
    }

    if (trimmed.startsWith('|') && index + 1 < lines.length && isTableSeparator(lines[index + 1])) {
      closeList();
      const headers = splitTableRow(line);
      index += 2;
      const rows = [];
      while (index < lines.length && lines[index].trim().startsWith('|')) {
        rows.push(splitTableRow(lines[index]));
        index += 1;
      }
      html.push(`<div class="table-wrap"><table class="cols-${headers.length}"><thead><tr>`);
      headers.forEach((cell) => html.push(`<th>${renderInline(cell)}</th>`));
      html.push('</tr></thead><tbody>');
      rows.forEach((row) => {
        html.push('<tr>');
        headers.forEach((_, cellIndex) => html.push(`<td>${renderInline(row[cellIndex] || '')}</td>`));
        html.push('</tr>');
      });
      html.push('</tbody></table></div>');
      continue;
    }

    if (trimmed.startsWith('>')) {
      closeList();
      const quote = [];
      while (index < lines.length && lines[index].trim().startsWith('>')) {
        quote.push(lines[index].trim().replace(/^>\s?/, ''));
        index += 1;
      }
      html.push(`<blockquote>${renderInline(quote.join(' '))}</blockquote>`);
      continue;
    }

    const unordered = trimmed.match(/^[-*+]\s+(.+)$/);
    const ordered = trimmed.match(/^\d+[.)]\s+(.+)$/);
    if (unordered || ordered) {
      const nextType = ordered ? 'ol' : 'ul';
      if (listType !== nextType) {
        closeList();
        listType = nextType;
        listStart = ordered ? Number.parseInt(trimmed, 10) : 1;
        html.push(listType === 'ol' && listStart !== 1 ? `<ol start="${listStart}">` : `<${listType}>`);
      }
      html.push(`<li>${renderInline((unordered || ordered)[1])}</li>`);
      index += 1;
      continue;
    }

    closeList();
    const paragraph = [trimmed];
    index += 1;
    while (index < lines.length) {
      const candidate = lines[index].trim();
      if (!candidate || candidate.startsWith('#') || candidate.startsWith('```') || candidate.startsWith('>') || candidate.startsWith('|') || /^[-*+]\s+/.test(candidate) || /^\d+[.)]\s+/.test(candidate) || /^---+$/.test(candidate)) break;
      paragraph.push(candidate);
      index += 1;
    }
    html.push(`<p>${renderInline(paragraph.join(' '))}</p>`);
  }

  closeList();
  if (codeFence) html.push(`<pre><code>${escapeHtml(codeLines.join('\n'))}</code></pre>`);
  return html.join('\n');
}

function stripReportTitle(markdown) {
  const lines = markdown.replaceAll('\r', '').split('\n');
  if (lines[0]?.startsWith('# ')) lines.shift();
  while (lines[0] !== undefined && lines[0].trim() === '') lines.shift();
  return lines.join('\n');
}

function demoteHeadings(markdown, amount = 1) {
  return markdown.replace(/^(#{1,5})(\s+)/gm, (match, hashes, spaces) => `${'#'.repeat(Math.min(6, hashes.length + amount))}${spaces}`);
}

const scoreRows = [
  ['Use case', 30, 29, 30],
  ['Safety & custody', 25, 24, 25],
  ['Craft', 20, 19, 20],
  ['Reproducibility', 15, 14, 15],
  ['Showcase', 10, 10, 10],
];
const currentScoreTotal = scoreRows.reduce((sum, row) => sum + row[2], 0);
const targetScoreTotal = scoreRows.reduce((sum, row) => sum + row[3], 0);

const scoreChart = scoreRows.map(([name, weight, current, target]) => `
  <div class="score-row">
    <div class="score-label"><strong>${name}</strong><span>${current}/${weight} now · ${target}/${weight} target</span></div>
    <div class="score-track"><span class="score-current" style="width:${(current / weight) * 100}%"></span></div>
    <div class="score-track target"><span class="score-target" style="width:${(target / weight) * 100}%"></span></div>
  </div>`).join('');

const architectureFigure = `
  <figure class="architecture">
    <div class="figure-title">End-to-end intent and trust flow</div>
    <div class="flow-row">
      <div class="node human"><b>Customer</b><span>ZeroClaw channel</span></div>
      <div class="arrow">→</div>
      <div class="node agent"><b>ZeroClaw agent</b><span>selects bounded tools</span></div>
      <div class="arrow">→</div>
      <div class="node safe"><b>kiosk-charge</b><span>offline Solana Pay intent</span></div>
      <div class="arrow">→</div>
      <div class="node human"><b>Customer wallet</b><span>customer signs</span></div>
    </div>
    <div class="flow-row second">
      <div class="node chain"><b>Solana</b><span>USDC transfer + evidence</span></div>
      <div class="arrow reverse">←</div>
      <div class="node read"><b>kiosk-watch</b><span>read-only verification</span></div>
      <div class="arrow reverse">←</div>
      <div class="node agent"><b>ZeroClaw SOP</b><span>route verified verdict</span></div>
      <div class="arrow">→</div>
      <div class="node hardware"><b>Relay / sensor</b><span>physical edge</span></div>
    </div>
    <div class="flow-row third">
      <div class="node hardware"><b>Observed event</b><span>sale or reading</span></div>
      <div class="arrow">→</div>
      <div class="node safe"><b>kiosk-attest</b><span>unsigned memo message</span></div>
      <div class="arrow">→</div>
      <div class="node human"><b>External signer</b><span>policy + private key</span></div>
      <div class="arrow">→</div>
      <div class="node chain"><b>Solana</b><span>landed attestation</span></div>
    </div>
    <figcaption>The three plugins, structured watcher output, trusted charge persistence, and one host-local exclusive claim are implemented. The external orchestration/recovery driver, signer/submission loop, actuator, and sensor remain integration work.</figcaption>
  </figure>`;

const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ProofKiosk System Handbook</title>
<style>
  :root {
    --blue: #2E74B5;
    --deep-blue: #173A5E;
    --ink: #17212B;
    --muted: #5B6875;
    --light-blue: #E8EEF5;
    --light-gray: #F2F4F7;
    --callout: #F4F6F9;
    --green: #26734D;
    --amber: #8A5A00;
    --red: #9B1C1C;
    --gold: #B7791F;
    --border: #CCD6E0;
  }
  @page { size: Letter portrait; margin: 1in; }
  * { box-sizing: border-box; }
  html { font-size: 11pt; }
  body {
    margin: 0;
    color: var(--ink);
    font-family: Calibri, Carlito, Arial, sans-serif;
    font-size: 11pt;
    line-height: 1.25;
    text-rendering: optimizeLegibility;
  }
  .cover {
    min-height: 8.65in;
    display: flex;
    flex-direction: column;
    justify-content: center;
    text-align: center;
    break-after: page;
  }
  .cover .kicker { color: var(--gold); font-size: 10pt; font-weight: 700; letter-spacing: .16em; text-transform: uppercase; margin-bottom: 18pt; }
  .cover h1 { color: var(--deep-blue); font-size: 30pt; line-height: 1.05; margin: 0 0 10pt; break-before: auto; }
  .cover .subtitle { color: #335B77; font-size: 15pt; line-height: 1.3; margin: 0 auto 28pt; max-width: 6in; }
  .cover .rule { width: 1.1in; height: 3px; background: var(--gold); margin: 0 auto 30pt; }
  .cover .meta { color: var(--muted); font-size: 10pt; line-height: 1.55; }
  .cover .badge { display: inline-block; border: 1px solid var(--border); border-radius: 999px; padding: 5pt 10pt; margin: 5pt 3pt 0; color: var(--deep-blue); background: #FAFBFC; font-size: 9pt; font-weight: 700; }
  .front-page { break-after: page; }
  .part-title { break-before: page; color: var(--deep-blue); border-top: 5px solid var(--gold); padding-top: 16pt; margin-top: 0; font-size: 22pt; }
  h1 { color: var(--blue); font-size: 16pt; line-height: 1.18; margin: 18pt 0 10pt; break-before: page; break-after: avoid; }
  h2 { color: var(--blue); font-size: 13pt; line-height: 1.2; margin: 14pt 0 7pt; break-after: avoid; }
  h3 { color: #1F4D78; font-size: 12pt; line-height: 1.2; margin: 10pt 0 5pt; break-after: avoid; }
  h4, h5, h6 { color: var(--deep-blue); font-size: 11pt; line-height: 1.2; margin: 8pt 0 4pt; break-after: avoid; }
  p { margin: 0 0 6pt; orphans: 3; widows: 3; }
  a { color: #155B96; text-decoration: none; border-bottom: 0.5px solid #AFC8DD; }
  strong { color: #111820; }
  code { font-family: "SFMono-Regular", Consolas, "Liberation Mono", monospace; font-size: 9pt; background: #F0F3F6; border-radius: 3px; padding: 1pt 2.5pt; overflow-wrap: anywhere; }
  pre { background: #17212B; color: #EDF3F8; border-radius: 6px; padding: 10pt 12pt; font-family: "SFMono-Regular", Consolas, monospace; font-size: 8.5pt; line-height: 1.28; white-space: pre-wrap; overflow-wrap: anywhere; break-inside: avoid; margin: 8pt 0 10pt; }
  pre code { color: inherit; background: transparent; padding: 0; font-size: inherit; }
  ul, ol { margin: 3pt 0 8pt 0.375in; padding-left: 0; }
  li { margin: 0 0 4pt; padding-left: 2pt; }
  blockquote { margin: 9pt 0; padding: 8pt 11pt; border-left: 4px solid var(--gold); background: #FFF9ED; color: #473B28; break-inside: avoid; }
  hr { border: 0; border-top: 1px solid var(--border); margin: 14pt 0; }
  .table-wrap { margin: 8pt 0 12pt; width: 100%; }
  table { border-collapse: collapse; table-layout: fixed; width: 100%; font-size: 8.6pt; line-height: 1.22; }
  thead { display: table-header-group; }
  tr { break-inside: avoid; }
  th { background: var(--light-blue); color: var(--deep-blue); font-weight: 700; text-align: left; vertical-align: middle; padding: 6pt 7pt; border: 0.6px solid #AEBFCC; }
  td { vertical-align: middle; padding: 6pt 7pt; border: 0.6px solid var(--border); overflow-wrap: anywhere; }
  tbody tr:nth-child(even) td { background: #FAFBFC; }
  td:nth-child(2):last-child, th:nth-child(2):last-child { text-align: left; }
  table.cols-2 th:first-child, table.cols-2 td:first-child { width: 28%; }
  table.cols-3 th:first-child, table.cols-3 td:first-child { width: 24%; }
  table.cols-3 th:nth-child(2), table.cols-3 td:nth-child(2) { width: 28%; }
  table.cols-6, table.cols-7 { table-layout: auto; font-size: 7.2pt; line-height: 1.16; }
  table.cols-6 th, table.cols-6 td, table.cols-7 th, table.cols-7 td { padding: 4pt; }
  .lead { font-size: 13pt; line-height: 1.45; color: var(--deep-blue); margin: 0 0 16pt; }
  .toc { columns: 2; column-gap: 30pt; }
  .toc a { display: block; break-inside: avoid; padding: 4pt 0; border-bottom: 1px dotted #B7C3CD; color: var(--deep-blue); }
  .toc span { color: var(--muted); font-size: 9pt; display: block; }
  .status-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 9pt; margin: 10pt 0 14pt; }
  .status-card { border: 1px solid var(--border); border-radius: 6px; padding: 9pt; break-inside: avoid; }
  .status-card strong { display: block; font-size: 11pt; margin-bottom: 3pt; }
  .status-card span { display: block; color: var(--muted); font-size: 9pt; line-height: 1.3; }
  .status-card.good { border-top: 4px solid var(--green); }
  .status-card.warn { border-top: 4px solid var(--amber); }
  .status-card.bad { border-top: 4px solid var(--red); }
  .scorecard { background: var(--callout); border: 1px solid var(--border); border-radius: 7px; padding: 11pt 13pt; margin: 10pt 0 14pt; break-inside: avoid; }
  .score-total { display: flex; justify-content: space-between; align-items: baseline; border-bottom: 1px solid var(--border); padding-bottom: 8pt; margin-bottom: 8pt; }
  .score-total b { color: var(--deep-blue); font-size: 18pt; }
  .score-total .target-total { color: var(--green); }
  .score-row { display: grid; grid-template-columns: 1.65in 1fr; column-gap: 8pt; row-gap: 3pt; margin-bottom: 7pt; }
  .score-label { grid-row: span 2; }
  .score-label strong, .score-label span { display: block; }
  .score-label span { color: var(--muted); font-size: 8.3pt; }
  .score-track { height: 6px; background: #DCE3EA; border-radius: 999px; overflow: hidden; }
  .score-current { display: block; height: 100%; background: var(--amber); }
  .score-target { display: block; height: 100%; background: var(--green); }
  .score-track.target { opacity: .8; }
  .callout { border-left: 4px solid var(--blue); background: var(--callout); padding: 9pt 11pt; margin: 10pt 0; break-inside: avoid; }
  .callout.risk { border-left-color: var(--red); background: #FFF5F5; }
  .callout b { color: var(--deep-blue); }
  .architecture { margin: 13pt 0 16pt; padding: 12pt; border: 1px solid var(--border); border-radius: 8px; background: #FBFCFD; break-inside: avoid; }
  .figure-title { font-size: 12pt; font-weight: 700; color: var(--deep-blue); margin-bottom: 9pt; }
  .flow-row { display: flex; align-items: stretch; justify-content: center; gap: 5pt; margin-bottom: 7pt; }
  .node { flex: 1; min-width: 0; border: 1px solid var(--border); border-radius: 5px; padding: 7pt 5pt; text-align: center; background: white; }
  .node b, .node span { display: block; }
  .node b { font-size: 9.2pt; color: var(--deep-blue); }
  .node span { font-size: 7.5pt; color: var(--muted); margin-top: 2pt; }
  .node.safe { border-top: 3px solid var(--green); }
  .node.read { border-top: 3px solid var(--blue); }
  .node.chain { border-top: 3px solid #7C3AED; }
  .node.hardware { border-top: 3px solid var(--amber); }
  .node.human { border-top: 3px solid #53667A; }
  .node.agent { border-top: 3px solid #CC3D6B; }
  .arrow { align-self: center; font-size: 14pt; color: var(--muted); flex: 0 0 auto; }
  figcaption { color: var(--muted); font-size: 8.2pt; line-height: 1.3; margin-top: 6pt; }
  .scope-note { color: var(--muted); font-size: 9pt; font-style: italic; }
  .page-break { break-before: page; }
  @media print {
    a { color: #155B96; }
    .cover, .front-page { page-break-after: always; }
  }
</style>
</head>
<body>
  <section class="cover">
    <div class="kicker">Technical handbook · architecture review · bounty readiness</div>
    <h1>ProofKiosk</h1>
    <div class="subtitle">A file-by-file guide to the ZeroClaw, Solana Pay, hardware, attestation, and DePIN system</div>
    <div class="rule"></div>
    <div class="meta">
      Repository snapshot <strong>${snapshotLabel}</strong> · refreshed 2 August 2026<br>
      Covers ${trackedFileCount} Git-tracked files, 225 repository tests plus one exact-host integration test and a shell host-infra regression, three WASM components, SOPs, hardware, security, and competition fit
    </div>
    <div>
      <span class="badge">Rust 2021</span>
      <span class="badge">wasm32-wasip2</span>
      <span class="badge">ZeroClaw</span>
      <span class="badge">Solana Pay + USDC</span>
      <span class="badge">DePIN-adjacent</span>
    </div>
  </section>

  <section class="front-page">
    <h1>How to use this handbook</h1>
    <p class="lead">Read the first six pages for the mental model and current score. Continue into the ecosystem chapters to understand ZeroClaw, Solana and DePIN. Use the repository atlas as the definitive file-by-file map when working in the code.</p>
    <div class="toc">
      <a href="#executive-summary">Executive summary<span>What exists, what works, and what does not</span></a>
      <a href="#architecture">System architecture<span>Money, data, trust and hardware paths</span></a>
      <a href="#ranking">Superteam ranking<span>Official weights, current score, target score</span></a>
      <a href="#part-ecosystem">Part I — Ecosystem foundations<span>ZeroClaw, Solana Pay, Solana and DePIN</span></a>
      <a href="#part-repository">Part II — Historical repository atlas<span>Pre-hardening file-by-file snapshot; not current evidence</span></a>
      <a href="#part-competition">Part III — Final readiness<span>Current evidence, score, residuals, and path to 100</span></a>
      <a href="#sources">Sources<span>Primary documentation and audited local code</span></a>
    </div>
    <div class="callout"><b>Scope.</b> “Every file” means every Git-tracked source, test, configuration, SOP, script, WIT, documentation and legal/build input in the audited commit. Ignored build outputs such as <code>target/</code>, <code>.vercel/</code> and staged binaries are discussed as artifacts rather than treated as authored source.</div>
    <div class="callout risk"><b>Interpretation rule.</b> Repository behavior wins over marketing copy. Where the static site, README, SOPs and code disagree, this handbook states the code-backed result and calls out the mismatch.</div>
  </section>

  <section id="executive-summary">
    <h1>Executive summary</h1>
    <p class="lead">ProofKiosk is a security-first reference system for letting an AI agent request USDC, verify payment on Solana, conditionally operate physical hardware, and prepare tamper-evident records without giving the model a spendable key.</p>
    <div class="status-grid">
      <div class="status-card good"><strong>Built and verified</strong><span>Three Rust/WASM plugins, shared Solana core, 225 passing repository tests, exact pinned-host execution, immutable quote/economics validation, one exclusive host-local claim, authenticated heartbeats, clean Clippy/rustfmt, successful WASM builds, zero HTTP imports in charge, and a finalized reference-bearing localnet transfer.</span></div>
      <div class="status-card warn"><strong>Integration still required</strong><span>Raw host-direct paid-result driver, actuator recovery journal, external signer/submission, sensor/relay/notification tools, and public-devnet physical evidence.</span></div>
      <div class="status-card bad"><strong>Production blockers</strong><span>At the exact ZeroClaw pin, headless deterministic execution self-dispatches only capability steps; ordinary plugins need an external driver. No relay/sensor adapter, signer, or exactly-once physical recovery state machine is shipped.</span></div>
    </div>
    <p><strong>What has been made:</strong> a substantial T0/T1 custody architecture, reusable Solana substrate, and trusted host-local handoff/claim boundary—not merely a landing page. <strong>What is being built:</strong> the external driver, signer, actuator/sensor adapters, and crash-recovery state machine that turn those components into a physical kiosk.</p>
    <p><strong>DePIN status:</strong> the project is DePIN-adjacent today. It connects payments, sensing and physical actuation to Solana, but it is not yet a decentralized network of independent infrastructure operators with shared service discovery, useful-work verification and operator rewards.</p>
  </section>

  <section id="architecture">
    <h1>System architecture at a glance</h1>
    ${architectureFigure}
    <div class="callout"><b>Core safety idea.</b> The customer signs the payment in their own wallet and funds move directly to an operator-configured merchant. The agent proposes and verifies; it does not possess the till.</div>
    <div class="callout risk"><b>Current integration reality.</b> Payment and heartbeat plugins emit structured JSON and authenticate the relevant signer/device facts. The compatible host is exact commit <code>e112ce6b5ccdac9e1cb166bab217e730dd7e24c2</code> (source version 0.8.2) with <code>plugins-wasm-cranelift</code>. Exact-host CI uses deterministic local JSON-RPC fixtures to execute valid charge, paid-watch, and unsigned-attest business paths, asserts attestation <code>minContextSlot</code>, and passes real host-direct charge and paid-watch results through immutable quote validation, one exclusive claim, and duplicate-claim rejection. Public-Devnet host-direct evidence is still absent. Headless ordinary plugin steps still need an external driver; relay, sensor, recovery, and signer paths remain absent.</div>
  </section>

  <section id="ranking">
    <h1>Superteam ZeroClaw ranking snapshot</h1>
    <p>The live listing showed <strong>91 submissions</strong>, <strong>seven paid placements</strong>, and a <strong>5,000 USDG</strong> pool. Because there are no public judge scores, an exact ordinal rank would be fabricated. The honest result is an acceptance-gate decision plus a rubric score.</p>
    <div class="scorecard">
      <div class="score-total"><span><strong>Submission engineering</strong><br>Assumes a perfect demo video; excludes X/social execution</span><b>${currentScoreTotal} / 100</b></div>
      <div class="score-total"><span><strong>Path to full marks</strong><br>Close the six residual code/infrastructure evidence gaps</span><b class="target-total">${targetScoreTotal} / 100</b></div>
      ${scoreChart}
    </div>
    <p><strong>Assessment:</strong> approximately 96/100 for submission engineering under the user's perfect-video assumption, with no fabricated ordinal placement. Production physical-system readiness is materially lower because the trusted actuator/signer/sensor recovery driver is not shipped. The remaining path to 100 is summarized in <code>docs/FINAL-READINESS-AUDIT.md</code>.</p>
  </section>

  <h1 class="part-title" id="part-ecosystem">Part I — ZeroClaw, Solana and DePIN foundations</h1>
  ${markdownToHtml(stripReportTitle(ecosystem), 'ecosystem')}

  <h1 class="part-title" id="part-repository">Part II — Historical pre-hardening repository atlas</h1>
  <div class="callout risk"><b>Historical snapshot—do not use as current evidence.</b> This imported file-by-file atlas predates the final hardening loop and intentionally preserves the earlier audit trail, including obsolete 107-test counts and bugs that are now fixed. Current behavior, counts, sizes, integration evidence, and residuals are stated in the executive summary, README, security documents, runbook, and Part III. Regenerate the atlas before using its line-level findings as a present-tense claim.</div>
  ${markdownToHtml(stripReportTitle(atlas), 'atlas')}

  <h1 class="part-title" id="part-competition">Part III — Final readiness audit and path to 100</h1>
  ${markdownToHtml(stripReportTitle(competition), 'competition')}

  <section id="sources">
    <h1>Source and confidence note</h1>
    <p>Local implementation claims were refreshed against repository snapshot <code>${snapshotLabel}</code>. A <code>+working-tree</code> suffix means the handbook includes uncommitted tracked changes, so record a clean commit hash before submission. The audit target is 213 Rust tests plus 12 Node trusted-boundary tests, rustfmt, Clippy with warnings denied, all three release WASM builds, one shell host-infrastructure regression, and one separate exact pinned ZeroClaw runtime integration test. A separate-customer localnet SPL transfer with a Solana Pay reference was submitted, finalized, and independently validated; that harness is not the plugin verifier. No public-devnet host-direct successful watch/attest call, signer submission, or GPIO device was exercised.</p>
    <p>External technical claims use primary sources: the official Superteam listing, the official ZeroClaw repository/docs at pinned commits, Solana and Solana Pay documentation, the SPL Memo specification, Circle's USDC address registry, and first-party DePIN project documentation. Competition placement is explicitly an inference; only the sponsor can assign an official score or rank.</p>
  </section>
</body>
</html>`;

fs.writeFileSync(htmlPath, html, 'utf8');

const executablePath = process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath });
  const page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
  await page.goto(pathToFileURL(htmlPath).href, { waitUntil: 'networkidle' });
  await page.emulateMedia({ media: 'print' });
  await page.pdf({
    path: pdfPath,
    format: 'Letter',
    printBackground: true,
    preferCSSPageSize: true,
    displayHeaderFooter: true,
    headerTemplate: '<div style="box-sizing:border-box;width:100%;font-size:8px;color:#718096;padding:0 0.72in;font-family:Arial,sans-serif;">ProofKiosk · System handbook</div>',
    footerTemplate: `<div style="box-sizing:border-box;width:100%;font-size:8px;color:#718096;padding:0 0.72in;font-family:Arial,sans-serif;display:flex;justify-content:space-between;"><span>Repository ${snapshotLabel} · refreshed 2 August 2026</span><span style="white-space:nowrap;"><span class="pageNumber"></span> / <span class="totalPages"></span></span></div>`,
  });
} finally {
  await browser?.close();
}

console.log(JSON.stringify({ htmlPath, pdfPath }));
