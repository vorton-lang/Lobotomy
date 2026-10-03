// Lobotomy streaming probe: does each CLI emit model text and tool output while a turn is still running?
// The agent runs one command that prints a line per second for 5 seconds; every stdout event is timestamped.
// Results are summarized in notes/harness-adapter.md §1.4.
//
// Usage: CODEX_BIN=<path to codex> node stream-probe.mjs claude|codex
// Real turns run on the user's subscriptions and leave test sessions in each CLI's history.
// The work dir defaults to <tmp>/lobotomy-stream-probe; each run writes <which>.jsonl there.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const which = process.argv[2];
if (which !== 'claude' && which !== 'codex') throw new Error('usage: node stream-probe.mjs claude|codex');
const probeDir = process.env.PROBE_DIR ?? path.join(os.tmpdir(), 'lobotomy-stream-probe');
fs.mkdirSync(probeDir, { recursive: true });
// Long-form path: Claude keys its project dirs by cwd, so avoid 8.3 short names on Windows.
const cwd = fs.realpathSync.native(probeDir);

const CMD = `node -e "let i=0;const t=setInterval(()=>{console.log('line '+(++i));if(i==5)clearInterval(t)},1000)"`;
const prompt = `Run this exact shell command once with your shell tool, then reply with just "ok":\n${CMD}`;

const [bin, args] = which === 'claude'
  ? ['claude', ['-p', '--output-format', 'stream-json', '--verbose', '--include-partial-messages',
      '--dangerously-skip-permissions', '--strict-mcp-config', '--model', process.env.CLAUDE_MODEL ?? 'haiku', prompt]]
  : [process.env.CODEX_BIN, ['exec', '--json', '--dangerously-bypass-approvals-and-sandbox', '--ignore-rules',
      '--skip-git-repo-check', '-c', 'model_reasoning_effort="low"', prompt]];

const t0 = performance.now();
const ts = () => ((performance.now() - t0) / 1000).toFixed(2).padStart(6);
const raw = [];
const child = spawn(bin, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
let buf = '';
child.stdout.on('data', d => {
  buf += d;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    raw.push(`${ts()} ${line}`);
    let ev; try { ev = JSON.parse(line); } catch { console.log(ts(), 'RAW', line.slice(0, 100)); continue; }
    console.log(ts(), describe(ev));
  }
});
child.stderr.on('data', d => process.stderr.write(d));
child.on('close', code => {
  console.log(ts(), 'exit', code);
  fs.writeFileSync(path.join(cwd, `${which}.jsonl`), raw.join('\n'));
});

function describe(ev) {
  let s = ev.type;
  if (ev.type === 'stream_event') s += `:${ev.event?.type}${ev.event?.delta?.type ? ':' + ev.event.delta.type : ''}`;
  if (ev.subtype) s += `:${ev.subtype}`;
  if (ev.item) s += ` item=${ev.item.type}/${ev.item.status ?? ''} out=${JSON.stringify(ev.item.aggregated_output ?? ev.item.text ?? '').slice(0, 70)}`;
  if (ev.type === 'assistant') s += ' ' + (ev.message?.content ?? []).map(c => c.type).join(',');
  if (ev.type === 'user') s += ' ' + JSON.stringify(ev.message?.content?.[0]?.content ?? '').slice(0, 70);
  return s;
}
