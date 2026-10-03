// Codex interrupt probe (Windows). For each way of ending a running turn, checks whether codex and
// the tool command it started survive, whether `codex exec resume` works afterwards, what the model
// says it saw, and what the rollout file recorded. Results go to notes/harness-adapter.md §1.4.
//
// Usage: CODEX_BIN=<codex.exe> WIN_PROC=<win-proc.exe> node interrupt-probe.mjs [mode ...]
// Modes: job-crash no-job-kill ctrl-break ctrl-c ctrl-c-isolated (default: all).
// ctrl-c-isolated also runs a bystander heartbeat that must survive. Real turns use the Codex subscription.
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CODEX = process.env.CODEX_BIN;
const WIN_PROC = process.env.WIN_PROC;
const modes = process.argv.slice(2).length ? process.argv.slice(2) : ['job-crash', 'no-job-kill', 'ctrl-break', 'ctrl-c', 'ctrl-c-isolated'];
const dir = process.env.PROBE_DIR ?? path.join(os.tmpdir(), 'lobotomy-interrupt-probe');
fs.mkdirSync(dir, { recursive: true });
const cwd = fs.realpathSync.native(dir);
const BASE = ['--json', '--dangerously-bypass-approvals-and-sandbox', '--ignore-rules', '--skip-git-repo-check', '-c', 'model_reasoning_effort="low"'];
const INTERRUPT_AFTER_S = 18;

const hbCmd = mode => `node -e "const f='hb-${mode}.txt';let n=0;const t=setInterval(()=>{require('fs').appendFileSync(f,Date.now()+'\\n');if(++n==40)clearInterval(t)},1000)"`;
const lines = f => (fs.existsSync(f) ? fs.readFileSync(f, 'utf8').split('\n').filter(Boolean).length : 0);
const sleep = ms => new Promise(r => setTimeout(r, ms));
// Resolve on process exit, not on pipe close: with std::process::Command on Windows, every child
// win-proc starts inherits its stdout pipe (all inheritable handles), which would keep the pipe open.
const runWinProc = args => new Promise(resolve => {
  const p = spawn(WIN_PROC, args, { cwd });
  let stdout = '';
  p.stdout.on('data', d => { stdout += d; });
  p.on('exit', status => setTimeout(() => resolve({ status, stdout }), 300));
});
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const procsMatching = needle => {
  const out = execFileSync('pwsh', ['-NoProfile', '-Command',
    `@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*${needle}*' -and $_.Name -ne 'pwsh.exe' } | Select-Object ProcessId,Name) | ConvertTo-Json -Compress`]).toString().trim();
  return out ? [].concat(JSON.parse(out)) : [];
};
const events = f => (fs.existsSync(f) ? fs.readFileSync(f, 'utf8').split('\n').filter(Boolean).map(l => { try { return JSON.parse(l); } catch { return { raw: l }; } }) : []);
const rolloutFor = tid => {
  const root = path.join(os.homedir(), '.codex', 'sessions');
  const walk = d => fs.readdirSync(d, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]);
  return walk(root).find(f => f.includes(tid));
};
const rolloutTail = (file, n = 14) => fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).slice(-n).map(l => {
  const r = JSON.parse(l); const p = r.payload ?? {};
  return `${r.type}:${p.type ?? ''}${p.name ? '(' + p.name + ')' : ''}${p.status ? '[' + p.status + ']' : ''}`;
});

const report = {};
for (const mode of modes) {
  const hb = path.join(cwd, `hb-${mode}.txt`);
  const out = path.join(cwd, `codex-${mode}.jsonl`);
  const bystanderHb = path.join(cwd, 'hb-bystander.txt');
  fs.rmSync(hb, { force: true });
  fs.rmSync(bystanderHb, { force: true });
  const prompt = `Run this exact shell command once with your shell tool and wait for it to finish (it takes about 40 seconds), then reply with just "ok":\n${hbCmd(mode)}`;
  console.error(`[${mode}] running`);
  const wp = await runWinProc([mode, String(INTERRUPT_AFTER_S), out, '--', CODEX, 'exec', ...BASE, prompt]);
  const tEvent = Date.now();
  const winProc = (wp.stdout ?? '').trim().split('\n');
  const codexPid = Number(winProc.find(l => l.startsWith('child_pid='))?.split('=')[1]);
  const hbAtEvent = lines(hb);
  const bystanderAtEvent = lines(bystanderHb);
  await sleep(6000);
  const hbAfter6s = lines(hb);
  const survivors = procsMatching(`hb-${mode}`);
  const bystanderAfter6s = lines(bystanderHb);
  for (const p of procsMatching('hb-bystander')) try { process.kill(p.ProcessId); } catch {}
  const r = {
    winProc, winProcExit: wp.status,
    codexAliveAfter6s: alive(codexPid),
    heartbeatLinesAtEvent: hbAtEvent, heartbeatLinesAfter6s: hbAfter6s,
    toolCommandSurvived: hbAfter6s > hbAtEvent,
    survivingProcesses: survivors,
  };
  if (mode === 'ctrl-c-isolated') r.bystanderKeptRunning = bystanderAfter6s > bystanderAtEvent;
  for (const p of survivors) try { process.kill(p.ProcessId); } catch {}
  if (alive(codexPid)) try { process.kill(codexPid); } catch {}

  const evs = events(out);
  r.codexEvents = evs.map(e => e.type + (e.item ? `:${e.item.type}/${e.item.status ?? ''}` : '') + (e.error ? `:${JSON.stringify(e.error).slice(0, 120)}` : '') + (e.message ? `:${String(e.message).slice(0, 120)}` : ''));
  r.codexStderrTail = fs.existsSync(out + '.err') ? fs.readFileSync(out + '.err', 'utf8').trim().split('\n').slice(-3) : [];
  const tid = evs.find(e => e.type === 'thread.started')?.thread_id;
  r.threadId = tid;
  if (tid) {
    const rollout = rolloutFor(tid);
    r.rolloutTailBeforeResume = rollout ? rolloutTail(rollout) : 'rollout not found';
    console.error(`[${mode}] resuming ${tid}`);
    const question = 'Do not run any commands or tools. Answer in at most two sentences: what was the last shell command you ran in this conversation, and did you receive its output?';
    const res = spawnSync(CODEX, ['exec', 'resume', tid, ...BASE, question], { cwd, encoding: 'utf8', timeout: 180000 });
    const rev = res.stdout.split('\n').filter(Boolean).map(l => { try { return JSON.parse(l); } catch { return {}; } });
    r.resume = {
      exit: res.status,
      types: rev.map(e => e.type),
      answer: rev.filter(e => e.item?.type === 'agent_message').map(e => e.item.text).join(' | '),
      errors: rev.filter(e => e.type === 'error' || e.type === 'turn.failed').map(e => JSON.stringify(e).slice(0, 200)),
      stderrTail: res.stderr.trim().split('\n').slice(-3),
    };
  }
  report[mode] = r;
  console.log(JSON.stringify({ mode, ...r }, null, 1));
}
fs.writeFileSync(path.join(cwd, 'report.json'), JSON.stringify(report, null, 2));
