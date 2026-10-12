// Angela's reply with the references the runtime renders (manager-actions.md §2.1): `[[quote:…]]`
// shows the user's own message with its time; `[[att:…]]` shows one line the runtime writes, with
// the version it is bound to and whether it still holds now, and opens the task in its project.

import { useEffect, useMemo } from 'react';
import { ChevronRight } from 'lucide-react';
import type { AttachmentView, QuotedMessage, Resolved } from '../api/types';
import { attachmentAccepted, attachmentExpired, attachmentLine, attachmentMetadata, attachmentWarning, clock, previewLine, splitReply, jsonObject, textOf } from '../format';
import { selectProject, useStore, wantForwards, wantRefs } from '../store';
import { ICON, Markdown } from './common';

export function Reply({ text }: { text: string }) {
  const parts = useMemo(() => splitReply(text), [text]);
  useEffect(() => {
    const ids = (kind: 'quote' | 'att') => parts.flatMap((p) => (p.kind === kind ? [p.id] : []));
    wantRefs(ids('quote'), ids('att'));
  }, [parts]);
  return (
    <>
      {parts.map((part, i) =>
        part.kind === 'text' ? (
          <Markdown key={i} text={part.text} />
        ) : part.kind === 'quote' ? (
          <Quote key={i} id={part.id} />
        ) : (
          <Attachment key={i} id={part.id} />
        ),
      )}
    </>
  );
}

const failed = <T extends object>(r: Resolved<T> | undefined): r is { error: string } => !!r && 'error' in r;

/** What the user chose that the message replies to (manager-actions.md §1.1), from the thread. */
export function ReplyLine({ id }: { id: string }) {
  const text = useStore((s) => {
    const item = s.thread.items.find((i) => i.id === id);
    if (item?.kind === 'agent_message') return textOf(jsonObject(item.content).text);
    return s.thread.messages.find((m) => m.id === id)?.body;
  });
  return <div className="reply-line">回复：「{text ? previewLine(text) : '较早的一条'}」</div>;
}

/** Where Angela forwarded the user's message: "已转给 …", with the context she attached (§1). */
export function ForwardTags({ id }: { id: string }) {
  const forwards = useStore((s) => s.forwards[id]);
  useEffect(() => wantForwards([id]), [id]);
  if (!forwards?.length) return null;
  return (
    <div className="forwards">
      {forwards.map((f, i) => {
        const where = f.task_title ? `${f.project_name} · 任务「${f.task_title}」` : f.project_name;
        const what = f.kind === 'task' ? '派成任务给' : '已转给';
        const context = f.contexts.map((c) => `${c.header.replace(/（item_[A-Za-z0-9_]+）$/, '')}${c.chosen ? '（你指定）' : ''}`).join('；');
        return (
          <button key={i} className="link forward" title={f.contexts.map((c) => c.header).join('；')} onClick={() => void selectProject(f.project, f.task_id)}>
            {what} {f.role}（{where}）{context && ` · 附上下文：${context}`}
          </button>
        );
      })}
    </div>
  );
}

function Quote({ id }: { id: string }) {
  const quote: Resolved<QuotedMessage> | undefined = useStore((s) => s.refs.quotes[id]);
  if (failed(quote)) return <p className="ref-error" title={id}>引用无效：{quote.error}</p>;
  if (!quote) return <p className="muted">正在读取引用的原话…</p>;
  return (
    <blockquote className="ref-quote" aria-label="你的原话">
      <div className="ref-head">你的原话 · {clock(quote.at)}</div>
      <Markdown text={quote.body} />
    </blockquote>
  );
}

function Attachment({ id }: { id: string }) {
  const attachment: Resolved<AttachmentView> | undefined = useStore((s) => s.refs.attachments[id]);
  if (failed(attachment)) return <p className="ref-error" title={id}>附件无效：{attachment.error}</p>;
  if (!attachment) return <p className="muted">正在读取附件…</p>;
  const expired = attachmentExpired(attachment);
  const warning = attachmentWarning(attachment);
  const task = attachment.task_id;
  return (
    <button
      className={`ref-attachment${expired ? ' expired' : ''}`}
      title={attachmentMetadata(attachment)}
      disabled={!task}
      onClick={() => task && void selectProject(attachment.project, task)}
    >
      <ChevronRight {...ICON} size={14} />
      {attachmentLine(attachment)}
      {warning && <span className="badge expired">{warning}</span>}
      {expired && <span className="badge expired">{expired}</span>}
      {!expired && attachmentAccepted(attachment) && <span className="badge passed accepted">已验收</span>}
    </button>
  );
}
