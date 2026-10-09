// A stand-in for `claude -p --output-format stream-json` in backend and GUI tests. It prints events
// in the format of Claude Code 2.1.283 (harness-adapter.md §1.4) and calls org_report on the real
// MCP service through the URL in the file of its `--mcp-config` argument.
//
// The input decides what happens:
//   FAKE:done     writes work.txt and reads it back, reports done, completes the turn;
//                 FAKE:text=<word> puts that word in the file instead of "hi"
//   FAKE:blocked  asks the user "空值怎么处理？" through org_report, then completes the turn
//   FAKE:wait=<ms>  waits that long before anything else
//   FAKE:fail     ends the turn with an error
//   FAKE:quota    the session limit: a refused rate_limit_event, Claude Code's made-up error
//                 message, and a result with status 429 (data-model.md §8.3)
//   FAKE:sleep    starts a command and waits; Ctrl+C ends it without a result
//   otherwise     completes the turn with a message
// Flags placed before Claude's own arguments change the process itself:
//   --fake-start-error  rejects its arguments: an error on stderr, nothing on stdout, exit code 1
// Every run writes the environment it sees to last-env.json and its arguments to last-args.json,
// inside .git when its cwd is a slot (so captures do not pick them up), else in its cwd.
import fs from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
const flags = new Set(args.filter(a => a.startsWith('--fake-')));
const diag = fs.existsSync('.git') ? '.git/' : '';
const watched = ['GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN', 'GH_CONFIG_DIR'];
fs.writeFileSync(`${diag}last-env.json`, JSON.stringify(Object.fromEntries(watched.map(k => [k, process.env[k] ?? null]))));
fs.writeFileSync(`${diag}last-args.json`, JSON.stringify(args));
const after = flag => {
  const i = args.indexOf(flag);
  return i < 0 ? undefined : args[i + 1];
};

if (flags.has('--fake-start-error')) {
  process.stderr.write('error: unknown option\n');
  process.exit(1);
}

const sessionId = after('--session-id') ?? after('--resume');
const config = after('--mcp-config');
const url = config ? JSON.parse(fs.readFileSync(config, 'utf8')).mcpServers.lobotomy.url : undefined;

const emit = event => process.stdout.write(JSON.stringify({ ...event, session_id: sessionId }) + '\n');
let input = '';
for await (const chunk of process.stdin) input += chunk;

emit({ type: 'system', subtype: 'init', cwd: process.cwd(), tools: ['PowerShell', 'Read', 'Write', 'Edit'], mcp_servers: [{ name: 'lobotomy', status: 'connected' }] });
const wait = /FAKE:wait=(\d+)/.exec(input);
if (wait) await new Promise(r => setTimeout(r, Number(wait[1])));

let messages = 0;
/** One assistant message of one block, as Claude streams it: partial text first, then the block. */
function message(block) {
  const id = `msg_fake_${messages++}`;
  emit({ type: 'stream_event', event: { type: 'message_start', message: { id, type: 'message', role: 'assistant', content: [] } }, parent_tool_use_id: null });
  emit({ type: 'stream_event', event: { type: 'content_block_start', index: 0, content_block: { type: block.type } }, parent_tool_use_id: null });
  if (block.type === 'text') {
    for (const piece of block.text.match(/.{1,4}/gsu) ?? []) {
      emit({ type: 'stream_event', event: { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: piece } }, parent_tool_use_id: null });
    }
  }
  emit({ type: 'assistant', message: { id, type: 'message', role: 'assistant', content: [block] }, parent_tool_use_id: null });
  emit({ type: 'stream_event', event: { type: 'content_block_stop', index: 0 }, parent_tool_use_id: null });
  emit({ type: 'stream_event', event: { type: 'message_stop' }, parent_tool_use_id: null });
}

let calls = 0;
/** A tool call and its result, in the next user message. */
function tool(name, toolInput, content, extra = {}) {
  const id = `toolu_fake_${calls++}`;
  message({ type: 'tool_use', id, name, input: toolInput });
  emit({ type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content, is_error: false }] }, parent_tool_use_id: null, ...extra });
}

/** Calls org_report on the runtime's MCP service, the way Claude calls an MCP tool. */
async function report(arguments_) {
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', 'mcp-protocol-version': '2025-06-18' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'org_report', arguments: arguments_ } }),
  });
  const reply = await response.json();
  tool('mcp__lobotomy__org_report', arguments_, reply.result.content);
}

message({ type: 'text', text: `got ${input.length} chars` });

if (input.includes('FAKE:fail')) {
  emit({ type: 'result', subtype: 'error_during_execution', is_error: true, result: 'boom' });
  process.exit(1);
}

if (input.includes('FAKE:quota')) {
  const limit = "You've hit your session limit · resets 12:20am (Asia/Tokyo)";
  emit({ type: 'rate_limit_event', rate_limit_info: { status: 'rejected', resetsAt: 1790868000, rateLimitType: 'five_hour' } });
  emit({ type: 'assistant', message: { id: 'msg_synthetic', model: '<synthetic>', role: 'assistant', content: [{ type: 'text', text: limit }] }, parent_tool_use_id: null, error: 'rate_limit' });
  emit({ type: 'result', subtype: 'success', is_error: true, api_error_status: 429, result: limit });
  process.exit(1);
}

if (input.includes('FAKE:sleep')) {
  process.on('SIGINT', () => process.exit(130));
  message({ type: 'tool_use', id: 'toolu_sleep', name: 'PowerShell', input: { command: 'sleep' } });
  await new Promise(r => setTimeout(r, 120_000));
  process.exit(0);
}

if (input.includes('FAKE:blocked')) {
  await report({ title: '需要你决定', body: '有两种做法。', status: 'blocked', blocked_on: '空值怎么处理？' });
}

if (input.includes('FAKE:done')) {
  const file = path.join(process.cwd(), 'work.txt');
  const created = !fs.existsSync(file);
  fs.writeFileSync(file, /FAKE:text=(\S+)/.exec(input)?.[1] ?? 'hi');
  tool('Write', { file_path: file, content: '…' }, `File created successfully at: ${file}`, { tool_use_result: { type: created ? 'create' : 'update', filePath: file } });
  tool('Read', { file_path: file }, `1\t${fs.readFileSync(file, 'utf8')}`);
  await report({ title: '完成', body: '写了 work.txt', status: 'done' });
}

message({ type: 'text', text: 'finished' });
emit({ type: 'result', subtype: 'success', is_error: false, result: 'finished', usage: { input_tokens: 1, output_tokens: 1 } });
