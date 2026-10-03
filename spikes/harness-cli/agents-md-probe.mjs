// Does Codex still load the user's global ~/.codex/AGENTS.md when run with --ignore-user-config?
// Asks the model (no tools) to quote the first heading of any AGENTS.md instructions it received,
// with and without the flag. Runs in an empty temp dir so no project AGENTS.md is involved.
// Results go to notes/harness-adapter.md §6.
//
// Usage: CODEX_BIN=<codex.exe> node agents-md-probe.mjs      (real turns on the Codex subscription)
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CODEX = process.env.CODEX_BIN;
const dir = process.env.PROBE_DIR ?? path.join(os.tmpdir(), 'lobotomy-agents-md-probe');
fs.mkdirSync(dir, { recursive: true });
const cwd = fs.realpathSync.native(dir);
const BASE = ['--json', '--dangerously-bypass-approvals-and-sandbox', '--ignore-rules', '--skip-git-repo-check', '-c', 'model_reasoning_effort="low"'];
const ASK = 'Do not run any tools. If your instructions include content from an AGENTS.md file (global or project), quote the first Markdown heading of that content exactly. Otherwise reply exactly: NONE';

for (const [label, extra] of [['default', []], ['ignore-user-config', ['--ignore-user-config']]]) {
  const r = spawnSync(CODEX, ['exec', ...BASE, ...extra, ASK], { cwd, encoding: 'utf8', timeout: 180000 });
  const evs = r.stdout.split('\n').filter(Boolean).map(l => { try { return JSON.parse(l); } catch { return {}; } });
  const answer = evs.filter(e => e.item?.type === 'agent_message').map(e => e.item.text).join(' | ');
  console.log(JSON.stringify({ label, exit: r.status, answer }));
}
