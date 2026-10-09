// One transcript item (data-model.md §7.2). A Lobotomy tool call shows its command record, which
// holds the arguments; other items show their own content.

import type { CommandRecord, Item, LiveItem, Text } from '../api/types';
import { asReport, commandSummary, elapsed, REPORT_LABEL, textOf, toolSubject, voidReport } from '../format';
import { Markdown, Output, useNow } from './common';

/** `fold`: the item is a report whose body a later message repeats; the body starts folded (#18). */
export function ItemView({
  item,
  role,
  commands,
  fold = false,
}: {
  item: Item;
  role: string;
  commands: Record<string, CommandRecord>;
  fold?: boolean;
}) {
  switch (item.kind) {
    case 'agent_message':
      return (
        <div className="message other">
          <div className="who">{role}</div>
          <Markdown text={textOf(item.content.text)} />
        </div>
      );
    case 'reasoning':
      return (
        <details className="item reasoning">
          <summary>思考</summary>
          <Markdown text={textOf(item.content.text)} />
        </details>
      );
    case 'command':
      return <CommandView command={item.content.command} output={item.content.output} exitCode={item.content.exit_code} />;
    case 'file_change':
      return (
        <div className="item file-change">
          {(item.content.changes ?? []).map((change) => (
            <div key={change.path}>
              <span className="badge">{change.kind}</span> <code>{change.path}</code>
            </div>
          ))}
        </div>
      );
    case 'mcp_call': {
      const report = asReport(item.command_id ? commands[item.command_id] : undefined);
      if (report) {
        const { title, body, status, blocked_on } = report.args;
        const voided = voidReport(report);
        return (
          <div className={`report ${voided ? 'void' : status}`}>
            <div className="report-head">
              <span className="badge">
                {REPORT_LABEL[status] ?? status}
                {voided && ' · 未生效'}
              </span>
              <strong>{title}</strong>
            </div>
            {voided && <p className="report-void">{voided}</p>}
            {body &&
              (fold ? (
                <details className="report-body">
                  <summary>报告全文</summary>
                  <Markdown text={body} />
                </details>
              ) : (
                <Markdown text={body} />
              ))}
            {blocked_on && <p className="blocked-on">{blocked_on}</p>}
          </div>
        );
      }
      const c = item.content;
      return (
        <details className="item">
          <summary>
            调用 {c.server}.{c.tool}
          </summary>
          <Output value={c.arguments} />
          <Output value={c.result ?? c.error} />
        </details>
      );
    }
    case 'tool_call':
      return <ToolCallView tool={item.content.tool} input={item.content.input} output={item.content.output} status={item.content.status} />;
    case 'web_search':
      return <div className="item muted">搜索：{textOf(item.content.query)}</div>;
    case 'todo_list':
      return (
        <ul className="item todo">
          {(item.content.items ?? []).map((t, i) => (
            <li key={i} className={t.completed ? 'done' : ''}>
              {t.completed ? '☑' : '☐'} {t.text}
            </li>
          ))}
        </ul>
      );
    case 'error':
      return <div className="item error">{textOf(item.content.message)}</div>;
    case 'input':
    case 'other':
      return (
        <details className="item">
          <summary>{item.kind}</summary>
          <Output value={item.content} />
        </details>
      );
  }
}

/**
 * A command and its output. Folded, it takes a line or two: a long command, such as a heredoc that
 * writes a file, shows its first line and its size, and the whole opens with the output (#14).
 */
function CommandView({
  command,
  output,
  exitCode,
  running,
}: {
  command: Text | undefined;
  output: Text | null | undefined;
  exitCode?: number | null;
  running?: React.ReactNode;
}) {
  const failed = exitCode !== null && exitCode !== undefined && exitCode !== 0;
  const { line, size } = commandSummary(command);
  return (
    <details className={`item command ${failed ? 'failed' : ''}`}>
      <summary>
        <code>$ {line}</code>
        {size && <span className="muted"> · {size}</span>}
        {running}
        {failed && <span className="error"> · 退出码 {exitCode}</span>}
      </summary>
      {size && <Output value={command} />}
      <Output value={output} />
    </details>
  );
}

/**
 * A tool of the harness's own, such as reading or searching files. Folded, it says which tool and
 * what it works on; opened, its input and what it returned.
 */
function ToolCallView({
  tool,
  input,
  output,
  status,
  running,
}: {
  tool: string;
  input: unknown;
  output?: Text | null;
  status?: string;
  running?: React.ReactNode;
}) {
  const subject = toolSubject(input);
  return (
    <details className={`item command ${status === 'failed' ? 'failed' : ''}`}>
      <summary>
        <code>{tool}</code>
        {subject && <span className="muted"> {subject}</span>}
        {running}
        {status === 'failed' && <span className="error"> · 失败</span>}
      </summary>
      <Output value={input} />
      <Output value={output} />
    </details>
  );
}

/**
 * An item still in progress: shown while it runs, never stored (frontend.md §3). A reply that
 * streams, as Claude's do, shows its text so far.
 */
export function LiveItemView({ item, role }: { item: LiveItem; role: string }) {
  const now = useNow();
  const since = <span className="running"> · 运行中 {elapsed(item.started_at, now)}</span>;
  switch (item.kind) {
    case 'command':
      return <CommandView command={item.content.command} output={item.content.output} running={since} />;
    case 'agent_message':
      return (
        <div className="message other live">
          <div className="who">{role}</div>
          <Markdown text={textOf(item.content.text)} />
        </div>
      );
    case 'tool_call':
      return <ToolCallView tool={item.content.tool} input={item.content.input} running={since} />;
    default:
      return (
        <div className="item muted">
          {item.kind}
          {since}
        </div>
      );
  }
}

export function RunningView({ role, since }: { role: string; since: number }) {
  const now = useNow();
  return <div className="item running">{role} 正在工作 · {elapsed(since, now)}</div>;
}
