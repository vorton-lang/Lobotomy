// Does changing the MCP URL between turns (a per-turn token) hurt Codex's prompt cache?
// Two threads, two turns each. Thread "same" resumes with the same URL; thread "changed" resumes
// with a new token in the URL. Compare the second turn's cached input share.
// Results go to notes/harness-adapter.md §1.4.
//
// Usage: CODEX_BIN=<codex.exe> node cache-probe.mjs      (real turns on the Codex subscription)
import { spawn } from 'node:child_process';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CODEX = process.env.CODEX_BIN;
const dir = process.env.PROBE_DIR ?? path.join(os.tmpdir(), 'lobotomy-cache-probe');
fs.mkdirSync(dir, { recursive: true });
const cwd = fs.realpathSync.native(dir);

// ---- minimal streamable-HTTP MCP server (same as spike.mjs); the URL token is logged per request ----
const seen = [];
const tools = [{
  name: 'org_report',
  description: 'Report your current status to the Lobotomy organization runtime.',
  inputSchema: { type: 'object', properties: { status: { type: 'string' } }, required: ['status'] },
}];
const handle = m => {
  const ok = result => ({ jsonrpc: '2.0', id: m.id, result });
  switch (m.method) {
    case 'initialize': return ok({ protocolVersion: m.params?.protocolVersion ?? '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'lobotomy-probe', version: '0.0.1' } });
    case 'ping': return ok({});
    case 'tools/list': return ok({ tools });
    case 'tools/call': return ok({ content: [{ type: 'text', text: 'recorded' }] });
    default: return { jsonrpc: '2.0', id: m.id, error: { code: -32601, message: `unsupported: ${m.method}` } };
  }
};
const server = http.createServer(async (req, res) => {
  const match = req.url.match(/^\/mcp\/([\w-]+)/);
  if (!match || req.method !== 'POST') return res.writeHead(match ? 405 : 404).end();
  let body = '';
  for await (const chunk of req) body += chunk;
  const msg = JSON.parse(body);
  const batch = Array.isArray(msg) ? msg : [msg];
  for (const m of batch) seen.push(`${match[1]}:${m.method}`);
  const replies = batch.filter(m => m.id !== undefined).map(handle);
  if (!replies.length) return res.writeHead(202).end();
  res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify(Array.isArray(msg) ? replies : replies[0]));
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const url = token => `http://127.0.0.1:${server.address().port}/mcp/${token}`;

const BASE = ['--json', '--dangerously-bypass-approvals-and-sandbox', '--ignore-rules', '--skip-git-repo-check', '-c', 'model_reasoning_effort="low"'];
// Async spawn: spawnSync would block the event loop and starve the in-process MCP server.
const run = args => new Promise(resolve => {
  const p = spawn(CODEX, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
  let out = '';
  p.stdout.on('data', d => { out += d; });
  p.on('close', code => {
    const evs = out.split('\n').filter(Boolean).map(l => { try { return JSON.parse(l); } catch { return {}; } });
    resolve({ code, tid: evs.find(e => e.type === 'thread.started')?.thread_id, usage: evs.find(e => e.type === 'turn.completed')?.usage });
  });
});

const result = {};
for (const variant of ['same', 'changed']) {
  const t1 = await run(['exec', ...BASE, '-c', `mcp_servers.lobotomy.url=${JSON.stringify(url(`${variant}-t1`))}`, 'Reply with just: ready']);
  const token2 = variant === 'same' ? `${variant}-t1` : `${variant}-t2`;
  const t2 = await run(['exec', 'resume', t1.tid, ...BASE, '-c', `mcp_servers.lobotomy.url=${JSON.stringify(url(token2))}`, 'Reply with just: done']);
  // turn.completed.usage is cumulative per thread (harness-adapter §1.3 rule 5), so take the difference.
  const d = k => (t2.usage?.[k] ?? 0) - (t1.usage?.[k] ?? 0);
  result[variant] = {
    t1: t1.usage, t2Delta: { input: d('input_tokens'), cached: d('cached_input_tokens') },
    t2CachedShare: d('input_tokens') ? +(d('cached_input_tokens') / d('input_tokens')).toFixed(3) : null,
    exitCodes: [t1.code, t2.code],
  };
}
result.mcpRequestsByToken = seen.reduce((m, s) => { const k = s.split(':')[0]; m[k] = (m[k] ?? 0) + 1; return m; }, {});
server.close();
console.log(JSON.stringify(result, null, 1));
