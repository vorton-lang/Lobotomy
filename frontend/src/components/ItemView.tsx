// One transcript item (data-model.md §7.2). A Lobotomy tool call shows its command record, which
// holds the arguments; other items show their own content.

import type { CommandRow, Item, LiveItem } from '../api/types';
import { elapsed, isBlob } from '../format';
import { Markdown, Output, useNow } from './common';

const REPORT_LABEL: Record<string, string> = { progress: '进展', blocked: '需要你决定', done: '完成' };

/** Reports the backend recorded without effect (report.rs `ReportEffect`). */
const VOID_REPORT: Record<string, string> = {
  late: '这次报告没有生效：它所属的执行轮已经结束，报告没有推进任务。',
  no_task: '这次报告没有生效：这个 turn 不属于任何任务，报告没有推进任务。它做的改动不会被验收或发布，要保留就建成任务。',
};

/** A command's first line shows at most this many characters while folded. */
const COMMAND_SUMMARY = 120;

export function ItemView({ item, commands }: { item: Item; commands: Record<string, CommandRow> }) {
  const c = item.content as Record<string, unknown>;
  switch (item.kind) {
    case 'agent_message':
      return (
        <div className="message other">
          <div className="who">Malkuth</div>
          <Markdown text={String(c.text ?? '')} />
        </div>
      );
    case 'reasoning':
      return (
        <details className="item reasoning">
          <summary>思考</summary>
          <Markdown text={String(c.text ?? '')} />
        </details>
      );
    case 'command':
      return <CommandView command={c.command} output={c.output} exitCode={c.exit_code as number | null} />;
    case 'file_change': {
      const changes = (c.changes as { path: string; kind: string }[] | null) ?? [];
      return (
        <div className="item file-change">
          {changes.map((change) => (
            <div key={change.path}>
              <span className="badge">{change.kind}</span> <code>{change.path}</code>
            </div>
          ))}
        </div>
      );
    }
    case 'mcp_call': {
      const record = item.command_id ? commands[item.command_id] : undefined;
      if (record?.name === 'org_report') {
        const args = record.args as { title?: string; body?: string; status?: string; blocked_on?: string };
        // What the report did, not what it asked for (#14).
        const voided = typeof record.result === 'string' ? VOID_REPORT[record.result] : undefined;
        return (
          <div className={`report ${voided ? 'void' : args.status}`}>
            <div className="report-head">
              <span className="badge">
                {REPORT_LABEL[args.status ?? ''] ?? args.status}
                {voided && ' · 未生效'}
              </span>
              <strong>{args.title}</strong>
            </div>
            {voided && <p className="report-void">{voided}</p>}
            {args.body && <Markdown text={args.body} />}
            {args.blocked_on && <p className="blocked-on">{args.blocked_on}</p>}
          </div>
        );
      }
      return (
        <details className="item">
          <summary>
            调用 {String(c.server)}.{String(c.tool)}
          </summary>
          <Output value={c.arguments} />
          <Output value={c.result ?? c.error} />
        </details>
      );
    }
    case 'web_search':
      return <div className="item muted">搜索：{String(c.query ?? '')}</div>;
    case 'todo_list': {
      const items = (c.items as { text: string; completed: boolean }[] | null) ?? [];
      return (
        <ul className="item todo">
          {items.map((t, i) => (
            <li key={i} className={t.completed ? 'done' : ''}>
              {t.completed ? '☑' : '☐'} {t.text}
            </li>
          ))}
        </ul>
      );
    }
    case 'error':
      return <div className="item error">{String(c.message ?? '')}</div>;
    default:
      return (
        <details className="item">
          <summary>{item.kind}</summary>
          <Output value={c} />
        </details>
      );
  }
}

/**
 * A command and its output. Folded, it takes a line or two: a long command, such as a heredoc that
 * writes a file, shows its first line and its size, and the whole opens with the output (#14).
 */
function CommandView({ command: raw, output, exitCode, running }: { command: unknown; output: unknown; exitCode?: number | null; running?: React.ReactNode }) {
  const failed = exitCode !== null && exitCode !== undefined && exitCode !== 0;
  // A command too large for the database is in the blob store; its head starts it.
  const command = typeof raw === 'string' ? raw : isBlob(raw) ? raw.head : String(raw ?? '');
  const size = isBlob(raw) ? raw.size : command.length;
  const lines = command.split('\n');
  const long = isBlob(raw) || lines.length > 1 || command.length > COMMAND_SUMMARY;
  const first = lines[0].length > COMMAND_SUMMARY ? `${lines[0].slice(0, COMMAND_SUMMARY)}…` : lines[0];
  return (
    <details className={`item command ${failed ? 'failed' : ''}`}>
      <summary>
        <code>$ {long ? first : command}</code>
        {long && (
          <span className="muted">
            {' '}
            · {isBlob(raw) ? '' : `共 ${lines.length} 行，`}
            {size} 个字符
          </span>
        )}
        {running}
        {failed && <span className="error"> · 退出码 {exitCode}</span>}
      </summary>
      {long && <Output value={raw} />}
      <Output value={output} />
    </details>
  );
}

/** An item still in progress: shown while it runs, never stored (frontend.md §3). */
export function LiveItemView({ item }: { item: LiveItem }) {
  const now = useNow();
  const c = item.content;
  const since = <span className="running"> · 运行中 {elapsed(item.started_at, now)}</span>;
  if (item.kind === 'command') return <CommandView command={c.command} output={c.output} running={since} />;
  return (
    <div className="item muted">
      {item.kind}
      {since}
    </div>
  );
}

export function RunningView({ since }: { since: number }) {
  const now = useNow();
  return <div className="item running">Malkuth 正在工作 · {elapsed(since, now)}</div>;
}
