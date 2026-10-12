// One transcript item (data-model.md §7.2). A Lobotomy tool call shows its command record, which
// holds the arguments; other items show their own content.

import type { CommandRecord, Item, LiveItem } from '../api/types';
import { jsonObject, jsonArray, asReport, commandSummary, elapsed, previewLine, REPORT_LABEL, textOf, toolSubject, voidReport } from '../format';
import { setReplyTo, useStore } from '../store';
import { Markdown, Output, useNow } from './common';
import { Reply } from './Reply';
import { Author } from './Status';

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
  const content = jsonObject(item.content);
  switch (item.kind) {
    case 'agent_message':
      return <AgentMessage id={item.id} role={role} text={textOf(content.text)} />;
    case 'reasoning':
      return (
        <details className="item reasoning">
          <summary>思考</summary>
          <Markdown text={textOf(content.text)} />
        </details>
      );
    case 'command':
      return <CommandView command={content.command} output={content.output} exitCode={content.exit_code} />;
    case 'file_change':
      return (
        <div className="item file-change">
          {jsonArray(content.changes).map(jsonObject).map((change) => (
            <div key={textOf(change.path)}>
              <span className="badge">{textOf(change.kind)}</span> <code>{textOf(change.path)}</code>
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
      const c = content;
      return (
        <details className="item">
          <summary>
            调用 {textOf(c.server)}.{textOf(c.tool)}
          </summary>
          <Output value={c.arguments} />
          <Output value={c.result ?? c.error} />
        </details>
      );
    }
    case 'tool_call':
      return <ToolCallView tool={content.tool} input={content.input} output={content.output} status={content.status} />;
    case 'web_search':
      return <div className="item muted">搜索：{textOf(content.query)}</div>;
    case 'todo_list':
      return (
        <ul className="item todo">
          {jsonArray(content.items).map(jsonObject).map((t, i) => (
            <li key={i} className={t.completed ? 'done' : ''}>
              {t.completed ? '☑' : '☐'} {textOf(t.text)}
            </li>
          ))}
        </ul>
      );
    case 'error':
      return <div className="item error">{textOf(content.message)}</div>;
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
 * A reply. On 「全部」 it is Angela's, whose references the runtime renders; the user can choose it
 * as what the next message replies to (manager-actions.md §1.1).
 */
function AgentMessage({ id, role, text }: { id: string; role: string; text: string }) {
  const angela = useStore((s) => s.project === null);
  return (
    <div className="message other">
      <div className="who">
        <Author name={role} />
        {angela && (
          <button className="link reply-to" onClick={() => setReplyTo({ id, text: previewLine(text) })}>
            回复
          </button>
        )}
      </div>
      {angela ? <Reply text={text} /> : <Markdown text={text} />}
    </div>
  );
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
  command: unknown;
  output: unknown;
  exitCode?: unknown;
  running?: React.ReactNode;
}) {
  const failed = typeof exitCode === 'number' && exitCode !== 0;
  const { line, size } = commandSummary(command);
  return (
    <details className={`item command ${failed ? 'failed' : ''}`}>
      <summary>
        <code>$ {line}</code>
        {size && <span className="muted"> · {size}</span>}
        {running}
        {failed && <span className="error"> · 退出码 {typeof exitCode === 'number' ? exitCode : ''}</span>}
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
  tool: unknown;
  input: unknown;
  output?: unknown;
  status?: unknown;
  running?: React.ReactNode;
}) {
  const subject = toolSubject(input);
  return (
    <details className={`item command ${status === 'failed' ? 'failed' : ''}`}>
      <summary>
        <code>{textOf(tool)}</code>
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
  const content = jsonObject(item.content);
  switch (item.kind) {
    case 'command':
      return <CommandView command={content.command} output={content.output} running={since} />;
    case 'agent_message':
      return (
        <div className="message other live">
          <div className="who">
            <Author name={role} />
          </div>
          <Markdown text={textOf(content.text)} streaming />
        </div>
      );
    case 'tool_call':
      return <ToolCallView tool={content.tool} input={content.input} running={since} />;
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
