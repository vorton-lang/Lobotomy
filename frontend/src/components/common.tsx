import { useEffect, useRef, useState, type ReactNode } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView, lineNumbers } from '@codemirror/view';
import { useMarkdown } from '../markdown/render';
import { call, toast } from '../store';

/** Rendered Markdown; plain text until the worker has it ready. */
export function Markdown({ text }: { text: string }) {
  const html = useMarkdown(text);
  if (html === null) return <div className="markdown plain">{text}</div>;
  return <div className="markdown" dangerouslySetInnerHTML={{ __html: html }} />;
}

/** The current time, refreshed every second while mounted. */
export function useNow(active = true): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <div className="backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={wide ? 'modal wide' : 'modal'} role="dialog" aria-label={title}>
        <header>
          <h2>{title}</h2>
          <button className="ghost" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </header>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}

/** Read-only viewer for large text (frontend.md §4.1): CodeMirror handles millions of lines. */
export function TextViewer({ text }: { text: string }) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const view = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: text,
        extensions: [lineNumbers(), EditorState.readOnly.of(true), EditorView.editable.of(false), EditorView.lineWrapping],
      }),
    });
    return () => view.destroy();
  }, [text]);
  return <div className="viewer" ref={host} />;
}

/** Opens the full text of a field kept in the blob store. */
export function BlobButton({ hash, label = '查看全文' }: { hash: string; label?: string }) {
  const [text, setText] = useState<string | null>(null);
  const open = async () => {
    try {
      setText((await call<{ text: string }>('blob', { hash })).text);
    } catch (e) {
      toast(String(e));
    }
  };
  return (
    <>
      <button className="link" onClick={open}>
        {label}
      </button>
      {text !== null && (
        <Modal title="全文" onClose={() => setText(null)} wide>
          <TextViewer text={text} />
        </Modal>
      )}
    </>
  );
}

/** A short text, or its head and tail with a button for the whole. */
export function Output({ value }: { value: unknown }) {
  if (typeof value === 'object' && value !== null && 'blob' in value) {
    const blob = value as { blob: string; size: number; head: string; tail: string };
    return (
      <div className="output">
        <pre>{blob.head}</pre>
        <div className="elided">
          …中间省略 · <BlobButton hash={blob.blob} />
        </div>
        <pre>{blob.tail}</pre>
      </div>
    );
  }
  const text = typeof value === 'string' ? value : value == null ? '' : JSON.stringify(value, null, 2);
  if (!text) return null;
  return (
    <div className="output">
      <pre>{text}</pre>
    </div>
  );
}
