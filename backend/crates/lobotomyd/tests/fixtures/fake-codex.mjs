// A stand-in for `codex exec --json` in backend tests. It prints events in Codex's format and
// calls org_report on the real MCP service through the URL in its arguments.
//
// The input decides what happens:
//   FAKE:done   writes work.txt, reports done, completes the turn
//   FAKE:fail   reports a failed turn
//   FAKE:sleep  starts a turn and waits; Ctrl+C ends it without a turn end event
//   otherwise   completes the turn with a message
// Flags placed before Codex's own arguments change the process itself:
//   --fake-stderr-flood  writes 4 MiB to stderr first and waits until it is read
//   --fake-hang          never finishes on its own
// Every run writes the environment it sees to last-env.json in its cwd.
import fs from 'node:fs';
import crypto from 'node:crypto';

const args = process.argv.slice(2);
const flags = new Set(args.filter(a => a.startsWith('--fake-')));
const watched = ['GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN', 'GH_CONFIG_DIR'];
fs.writeFileSync('last-env.json', JSON.stringify(Object.fromEntries(watched.map(k => [k, process.env[k] ?? null]))));
const config = name => {
  const i = args.findIndex((a, j) => args[j - 1] === '-c' && a.startsWith(`${name}=`));
  return i < 0 ? undefined : JSON.parse(args[i].slice(name.length + 1));
};
const resumeAt = args.indexOf('resume');
const threadId = resumeAt >= 0 ? args[resumeAt + 1] : crypto.randomUUID();
const url = config('mcp_servers.lobotomy.url');
fs.writeFileSync('last-args.json', JSON.stringify(args));

const emit = event => process.stdout.write(JSON.stringify(event) + '\n');
let input = '';
for await (const chunk of process.stdin) input += chunk;

if (flags.has('--fake-stderr-flood')) await new Promise(r => process.stderr.write('x'.repeat(4 << 20), r));
if (flags.has('--fake-hang')) await new Promise(r => setTimeout(r, 600_000));

emit({ type: 'thread.started', thread_id: threadId });
emit({ type: 'turn.started' });
emit({ type: 'item.completed', item: { id: 'item_0', type: 'agent_message', text: `got ${input.length} chars` } });

if (input.includes('FAKE:fail')) {
  emit({ type: 'turn.failed', error: { message: 'boom' } });
  process.exit(1);
}

if (input.includes('FAKE:sleep')) {
  process.on('SIGINT', () => process.exit(130));
  emit({ type: 'item.started', item: { id: 'item_1', type: 'command_execution', command: 'sleep', aggregated_output: '', exit_code: null, status: 'in_progress' } });
  await new Promise(r => setTimeout(r, 120_000));
  process.exit(0);
}

if (input.includes('FAKE:done')) {
  fs.writeFileSync('work.txt', 'hi');
  const arguments_ = { title: '完成', body: '写了 work.txt', status: 'done' };
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', 'mcp-protocol-version': '2025-06-18' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'org_report', arguments: arguments_ } }),
  });
  const reply = await response.json();
  emit({
    type: 'item.completed',
    item: { id: 'item_2', type: 'mcp_tool_call', server: 'lobotomy', tool: 'org_report', arguments: arguments_, result: reply.result, error: null, status: 'completed' },
  });
}

emit({ type: 'item.completed', item: { id: 'item_9', type: 'agent_message', text: 'finished' } });
emit({ type: 'turn.completed', usage: { input_tokens: 1, cached_input_tokens: 0, output_tokens: 1 } });
