// Angela's reply with the references the runtime renders (manager-actions.md §2.1): `[[quote:…]]`
// shows the user's own message with its time; `[[att:…]]` shows one line the runtime writes, with
// the version it is bound to and whether it still holds now, and opens the task in its project.

import { useEffect, useMemo } from 'react';
import type { AttachmentView, QuotedMessage, Resolved } from '../api/types';
import { attachmentExpired, attachmentLine, clock, splitReply } from '../format';
import { selectProject, useStore, wantRefs } from '../store';
import { Markdown } from './common';

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

function Quote({ id }: { id: string }) {
  const quote: Resolved<QuotedMessage> | undefined = useStore((s) => s.refs.quotes[id]);
  if (failed(quote)) return <p className="ref-error">引用无效（{id}）：{quote.error}</p>;
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
  if (failed(attachment)) return <p className="ref-error">附件无效（{id}）：{attachment.error}</p>;
  if (!attachment) return <p className="muted">正在读取附件…</p>;
  const expired = attachmentExpired(attachment);
  const task = attachment.task_id;
  return (
    <button
      className={`ref-attachment${expired ? ' expired' : ''}`}
      disabled={!task}
      onClick={() => task && void selectProject(attachment.project, task)}
    >
      <span aria-hidden>▸ </span>
      {attachmentLine(attachment)}
      {expired && <span className="badge expired">{expired}</span>}
    </button>
  );
}
