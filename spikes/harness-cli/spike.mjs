// Lobotomy harness spike: drive Claude Code and Codex purely through their official CLIs,
// one process per turn, with an in-process HTTP MCP server standing in for the daemon.
// Results are summarized in notes/harness-adapter.md §1.4.
//
// Usage: CODEX_BIN=<path to codex> node spike.mjs [claude|codex]
// Real turns run on the user's subscriptions and leave test sessions in each CLI's history.
// Work dirs (repo/, elsewhere/, out/) go under SPIKE_DIR, default <tmp>/lobotomy-harness-spike.
import { spawn, execFileSync } from 'node:child_process';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';

const spikeDir = process.env.SPIKE_DIR ?? path.join(os.tmpdir(), 'lobotomy-harness-spike');
fs.mkdirSync(spikeDir, { recursive: true });
// Long-form path: Claude keys its project dirs by cwd, so avoid 8.3 short names on Windows.
const ROOT = fs.realpathSync.native(spikeDir);
const REPO = path.join(ROOT, 'repo');
const ELSEWHERE = path.join(ROOT, 'elsewhere');
const OUT = path.join(ROOT, 'out');
const CODEX = process.env.CODEX_BIN;
const CLAUDE_MODEL = process.env.CLAUDE_MODEL ?? 'haiku';
const only = process.argv[2]; // 'claude' | 'codex' | undefined

for (const d of [ELSEWHERE, OUT]) fs.mkdirSync(d, { recursive: true });
if (!fs.existsSync(path.join(REPO, '.git'))) {
  fs.mkdirSync(REPO, { recursive: true });
  fs.writeFileSync(path.join(REPO, 'README.md'), '# spike repo\n');
  const git = (...a) => execFileSync('git', ['-c', 'user.name=spike', '-c', 'user.email=spike@local', ...a], { cwd: REPO });
  git('init', '-q'); git('add', '.'); git('commit', '-qm', 'init');
}

// ---- minimal streamable-HTTP MCP server; caller identity comes from the URL token ----
const mcpLog = [];
const tools = [{
  name: 'org_report',
  description: 'Report your current status to the Lobotomy organization runtime.',
  inputSchema: { type: 'object', properties: { status: { type: 'string' } }, required: ['status'] },
}];
function handle(token, m) {
  const ok = result => ({ jsonrpc: '2.0', id: m.id, result });
  switch (m.method) {
    case 'initialize':
      return ok({ protocolVersion: m.params?.protocolVersion ?? '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'lobotomy-spike', version: '0.0.1' } });
    case 'ping': return ok({});
    case 'tools/list': return ok({ tools });
    case 'tools/call':
      mcpLog.push({ token, tool: m.params?.name, args: m.params?.arguments });
      return ok({ content: [{ type: 'text', text: `recorded by lobotomy for caller "${token}"` }] });
    default: return { jsonrpc: '2.0', id: m.id, error: { code: -32601, message: `unsupported: ${m.method}` } };
  }
}
const server = http.createServer(async (req, res) => {
  const match = req.url.match(/^\/mcp\/([\w-]+)/);
  if (!match) return res.writeHead(404).end();
  if (req.method !== 'POST') return res.writeHead(405, { Allow: 'POST' }).end();
  let body = '';
  for await (const chunk of req) body += chunk;
  let msg;
  try { msg = JSON.parse(body); } catch { return res.writeHead(400).end(); }
  const batch = Array.isArray(msg) ? msg : [msg];
  const replies = batch.filter(m => m.id !== undefined).map(m => handle(match[1], m));
  for (const m of batch) mcpLog.push({ token: match[1], method: m.method });
  if (!replies.length) return res.writeHead(202).end();
  res.writeHead(200, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(Array.isArray(msg) ? replies : replies[0]));
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const mcpUrl = token => `http://127.0.0.1:${server.address().port}/mcp/${token}`;

// ---- one CLI turn = one process ----
async function run(label, cmd, args, cwd) {
  const t0 = performance.now();
  const child = spawn(cmd, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  let firstByteMs = null, buf = '', stderr = '';
  const lines = [];
  child.stdout.on('data', d => {
    firstByteMs ??= Math.round(performance.now() - t0);
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { lines.push(buf.slice(0, i)); buf = buf.slice(i + 1); }
  });
  child.stderr.on('data', d => { stderr += d; });
  const code = await new Promise(r => child.on('close', r));
  if (buf.trim()) lines.push(buf);
  fs.writeFileSync(path.join(OUT, `${label}.jsonl`), lines.join('\n'));
  if (stderr) fs.writeFileSync(path.join(OUT, `${label}.stderr.txt`), stderr);
  const events = lines.map(l => { try { return JSON.parse(l); } catch { return { _raw: l }; } });
  return { label, cwd: path.basename(cwd), code, firstByteMs, totalMs: Math.round(performance.now() - t0), events, stderr };
}

const clip = (s, n = 220) => (s && s.length > n ? s.slice(0, n) + '…' : s);
const counts = evs => evs.reduce((m, e) => {
  const k = [e.type, e.subtype ?? e.item?.type].filter(Boolean).join(':') || '(raw)';
  m[k] = (m[k] ?? 0) + 1; return m;
}, {});
const compactEvents = evs => evs.filter(e => /compact/i.test(JSON.stringify(e))).map(e => clip(JSON.stringify(e), 300));

function summarizeClaude(r) {
  const init = r.events.find(e => e.type === 'system' && e.subtype === 'init');
  const result = r.events.find(e => e.type === 'result');
  return {
    label: r.label, cwd: r.cwd, code: r.code, firstByteMs: r.firstByteMs, totalMs: r.totalMs,
    sessionId: init?.session_id ?? result?.session_id,
    model: init?.model,
    mcp: init?.mcp_servers?.filter(s => s.name === 'lobotomy'),
    hasOrgTool: init?.tools?.some(t => /org_report/.test(t)),
    text: clip(result?.result), isError: result?.is_error, usage: result?.usage,
    events: counts(r.events), compaction: compactEvents(r.events),
    stderr: clip(r.stderr, 400),
  };
}
function summarizeCodex(r) {
  const items = r.events.filter(e => e.type === 'item.completed').map(e => e.item);
  return {
    label: r.label, cwd: r.cwd, code: r.code, firstByteMs: r.firstByteMs, totalMs: r.totalMs,
    threadId: r.events.find(e => e.type === 'thread.started')?.thread_id,
    text: clip(items.filter(i => i?.type === 'agent_message').map(i => i.text).join(' | ')),
    commands: items.filter(i => i?.type === 'command_execution').map(i => clip(`${i.command} -> ${i.aggregated_output ?? ''}`, 200)),
    mcpCalls: items.filter(i => i?.type === 'mcp_tool_call').map(i => clip(JSON.stringify(i), 200)),
    usage: r.events.find(e => e.type === 'turn.completed')?.usage,
    errors: r.events.filter(e => e.type === 'error' || e.type === 'turn.failed').map(e => clip(JSON.stringify(e), 300)),
    events: counts(r.events), compaction: compactEvents(r.events),
    stderr: clip(r.stderr, 400),
  };
}

const report = { versions: {}, claude: [], codex: [] };
const ROLE = tag => `You are the role "${tag}" inside an organization runtime called Lobotomy. Your counterpart (the user) is the Manager. Begin every reply with the tag [${tag}].`;
const ASK_TAG = 'Without using any tools: what is your role tag? Answer in one line.';

if (only !== 'codex') {
  report.versions.claude = execFileSync('claude', ['--version']).toString().trim();
  const mcpCfg = path.join(OUT, 'claude-mcp.json');
  fs.writeFileSync(mcpCfg, JSON.stringify({ mcpServers: { lobotomy: { type: 'http', url: mcpUrl('tl-token') } } }));
  const sid = crypto.randomUUID();
  const base = ['-p', '--output-format', 'stream-json', '--verbose', '--dangerously-skip-permissions', '--model', CLAUDE_MODEL, '--mcp-config', mcpCfg];
  const turn = async (label, args, cwd = REPO) => {
    const s = summarizeClaude(await run(label, 'claude', [...base, ...args], cwd));
    report.claude.push(s); console.error(`[claude] ${label} code=${s.code} ${s.totalMs}ms`); return s;
  };
  await turn('c1-new', ['--session-id', sid, '--append-system-prompt', ROLE('TL-SPIKE'), 'Call the org_report tool with status "c1-ok". Then reply with one short sentence.']);
  await turn('c2-resume-other-cwd', ['--resume', sid, ASK_TAG], ELSEWHERE);
  await turn('c3-resume-no-role-arg', ['--resume', sid, ASK_TAG]);
  await turn('c4-compact', ['--resume', sid, '/compact']);
  await turn('c5-after-compact-no-role-arg', ['--resume', sid, ASK_TAG]);
  await turn('c6-after-compact-with-role-arg', ['--resume', sid, '--append-system-prompt', ROLE('TL-SPIKE'), ASK_TAG]);
  await turn('c7-fork', ['--resume', sid, '--fork-session', 'Reply with just: fork-ok']);
}

if (only !== 'claude') {
  report.versions.codex = execFileSync(CODEX, ['--version']).toString().trim();
  const toml = s => JSON.stringify(s);
  const base = ['--json', '--dangerously-bypass-approvals-and-sandbox', '--ignore-rules', '-c', 'model_reasoning_effort="low"'];
  const turn = async (label, args, cwd = REPO) => {
    const s = summarizeCodex(await run(label, CODEX, args, cwd));
    report.codex.push(s); console.error(`[codex] ${label} code=${s.code} ${s.totalMs}ms`); return s;
  };
  // Probe whether --thread-source takes free text; an enum rejects at arg parsing.
  await turn('x0-thread-source-probe', ['exec', '--thread-source', 'zz-probe', '--help'], ELSEWHERE);
  const x1 = await turn('x1-new', ['exec', ...base, '-C', REPO,
    '-c', `developer_instructions=${toml(ROLE('WORKER-SPIKE'))}`,
    '-c', `mcp_servers.lobotomy.url=${toml(mcpUrl('worker-token'))}`,
    'Call the org_report tool with status "x1-ok". Then run one shell command that prints the current working directory, and reply with that directory in one line.']);
  const tid = x1.threadId;
  if (tid) {
    await turn('x2-resume-other-cwd-no-role-arg', ['exec', 'resume', tid, ...base,
      'What is your role tag, and what is your current working directory? Run one shell command to check the directory. Answer in two lines.'], ELSEWHERE);
    await turn('x3-compact-low-limit', ['exec', 'resume', tid, ...base, '-c', 'model_auto_compact_token_limit=2000', 'Reply with just: compact-probe']);
    await turn('x4-after-compact-no-role-arg', ['exec', 'resume', tid, ...base, ASK_TAG]);
    await turn('x5-fork', ['exec', 'fork', tid, ...base, 'Reply with just: fork-ok']);
  }
}

report.mcpLog = mcpLog;
server.close();
fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 1));
