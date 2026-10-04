// A stand-in for `codex exec --json` in backend tests. It prints events in Codex's format and
// calls org_report on the real MCP service through the URL in its arguments.
//
// The input decides what happens:
//   FAKE:done   writes work.txt, reports done, completes the turn; with FAKE:text=<word> the
//               file holds that word instead of "hi"; with FAKE:nowrite it writes nothing
//   FAKE:wait=<ms>  waits that long before anything else, so a message can arrive meanwhile
//   FAKE:newthread  reports a new session id even when resuming, which the runtime cannot record
//   FAKE:managed  acts as a Codex whose administrator does not allow full access: given
//               --dangerously-bypass-approvals-and-sandbox, it exits at start with Codex's error
//               on stderr and nothing on stdout; otherwise the other modes go on
//   FAKE:longcmd  emits a command that writes a file with a heredoc of 405 lines (#14)
//   FAKE:fail   reports a failed turn
//   FAKE:sleep  starts a turn and waits; Ctrl+C ends it without a turn end event
//   FAKE:big    emits a command item with 200 KiB of output
//   FAKE:refused  emits an org_report call that Codex refused, then completes the turn
//   FAKE:many=…, FAKE:huge, …  synthetic turns for the performance baseline (fake-bench.mjs)
//   otherwise   completes the turn with a message
// Flags placed before Codex's own arguments change the process itself:
//   --fake-stderr-flood  writes 4 MiB to stderr first and waits until it is read
//   --fake-hang          never finishes on its own
//   --fake-start-error   rejects its arguments: an error and a backtrace on stderr, nothing on
//                        stdout, exit code 1 (#13)
// Every run writes the environment it sees to last-env.json and its arguments to last-args.json,
// inside .git when its cwd is a slot (so captures do not pick them up), else in its cwd.
import fs from 'node:fs';
import crypto from 'node:crypto';
import { bench } from './fake-bench.mjs';

const args = process.argv.slice(2);
const flags = new Set(args.filter(a => a.startsWith('--fake-')));
const diag = fs.existsSync('.git') ? '.git/' : '';
const watched = ['GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN', 'GH_CONFIG_DIR'];
fs.writeFileSync(`${diag}last-env.json`, JSON.stringify(Object.fromEntries(watched.map(k => [k, process.env[k] ?? null]))));
const config = name => {
  const i = args.findIndex((a, j) => args[j - 1] === '-c' && a.startsWith(`${name}=`));
  return i < 0 ? undefined : JSON.parse(args[i].slice(name.length + 1));
};
const resumeAt = args.indexOf('resume');
const url = config('mcp_servers.lobotomy.url');
fs.writeFileSync(`${diag}last-args.json`, JSON.stringify(args));

if (flags.has('--fake-start-error')) {
  const backtrace = Array.from({ length: 8 }, (_, i) => `  ${i}: <unknown>`).join('\n');
  process.stderr.write(`Error: mcp_servers.lobotomy.url = ${url} is not allowed here\n${backtrace}\n`);
  process.exit(1);
}

const emit = event => process.stdout.write(JSON.stringify(event) + '\n');
let input = '';
for await (const chunk of process.stdin) input += chunk;

if (input.includes('FAKE:managed') && args.includes('--dangerously-bypass-approvals-and-sandbox')) {
  // Codex 0.159.2's words (#13).
  process.stderr.write('Error: `approval_policy = "never"` cannot be used because requirements do not allow `sandbox_mode = "danger-full-access"`; Codex would fall back to read-only permissions with approvals disabled. Choose an `approval_policy` based on what you need, such as `on-request`, or choose an allowed sandbox mode.\n');
  process.exit(1);
}

if (flags.has('--fake-stderr-flood')) await new Promise(r => process.stderr.write('x'.repeat(4 << 20), r));
if (flags.has('--fake-hang')) await new Promise(r => setTimeout(r, 600_000));

const threadId = resumeAt >= 0 && !input.includes('FAKE:newthread') ? args[resumeAt + 1] : crypto.randomUUID();
emit({ type: 'thread.started', thread_id: threadId });
emit({ type: 'turn.started' });
const wait = /FAKE:wait=(\d+)/.exec(input);
if (wait) await new Promise((r) => setTimeout(r, Number(wait[1])));
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

await bench(input, emit);

if (input.includes('FAKE:big')) {
  const output = 'y'.repeat(200 * 1024);
  emit({ type: 'item.completed', item: { id: 'item_1', type: 'command_execution', command: 'build', aggregated_output: output, exit_code: 0, status: 'completed' } });
}

if (input.includes('FAKE:refused')) {
  const error = { message: 'MCP tool call requires approval, but approval policy is never' };
  const call = { id: 'item_2', type: 'mcp_tool_call', server: 'lobotomy', tool: 'org_report', arguments: { status: 'done' } };
  emit({ type: 'item.completed', item: { ...call, result: null, error, status: 'failed' } });
}

if (input.includes('FAKE:longcmd')) {
  const body = Array.from({ length: 402 }, (_, i) => `  line ${i} of the generated source;`).join('\n');
  const command = `cat > generated.txt <<'EOF'\n${body}\nheredoc-end-marker\nEOF`;
  emit({ type: 'item.completed', item: { id: 'item_long', type: 'command_execution', command, aggregated_output: '', exit_code: 0, status: 'completed' } });
}

if (input.includes('FAKE:done')) {
  if (!input.includes('FAKE:nowrite')) fs.writeFileSync('work.txt', /FAKE:text=(\S+)/.exec(input)?.[1] ?? 'hi');
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
