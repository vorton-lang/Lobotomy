import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { defaultKeymap } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { EditorView, keymap, lineNumbers } from '@codemirror/view';
import { bytes, firstLines, fits, isBlob, lastLines } from '../format';
import { useMarkdown } from '../markdown/render';
import { call, toast } from '../store';

/** Rendered Markdown; plain text until the worker has it ready. */
export function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const html = useMarkdown(text, streaming);
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

/**
 * A dialog over the whole window. It is rendered at the end of the body: inside a row of the
 * virtualized thread, the row's positioning would place and clip it (#15). Clicks inside do not
 * reach the components it was opened from.
 */
export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  return createPortal(
    <div
      className="backdrop"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
      onClick={(e) => e.stopPropagation()}
    >
      <div className={wide ? 'modal wide' : 'modal'} role="dialog" aria-modal="true" aria-label={title}>
        <header>
          <h2>{title}</h2>
          <button className="ghost" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </header>
        <div className="modal-body">{children}</div>
      </div>
    </div>,
    document.body,
  );
}

/**
 * Read-only viewer for large text (frontend.md §4.1): CodeMirror handles millions of lines. It
 * takes focus, and CodeMirror's own keys move and select over the whole text, not only the lines
 * it has drawn: Ctrl+A and Ctrl+C copy all of it, Ctrl+End goes to the end (#15). Editing keys do
 * nothing in a read-only state.
 */
function TextViewer({ text }: { text: string }) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const view = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: text,
        extensions: [lineNumbers(), EditorState.readOnly.of(true), keymap.of(defaultKeymap), EditorView.lineWrapping],
      }),
    });
    view.focus();
    return () => view.destroy();
  }, [text]);
  return <div className="viewer" ref={host} />;
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast(`已复制 ${text.length} 个字符`);
  } catch (e) {
    toast(`复制失败：${String(e)}`);
  }
}

/** Opens a whole text in the viewer. */
function FullText({ load, label = '查看全文' }: { load: () => Promise<string>; label?: string }) {
  const [text, setText] = useState<string | null>(null);
  const open = async () => {
    try {
      setText(await load());
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
          <div className="actions viewer-actions">
            <span className="muted">
              {text.split('\n').length} 行，{text.length} 个字符
            </span>
            <button onClick={() => copy(text)}>复制全文</button>
          </div>
          <TextViewer text={text} />
        </Modal>
      )}
    </>
  );
}

/** Opens the full text of a field kept in the blob store. */
export function BlobButton({ hash, label }: { hash: string; label?: string }) {
  return <FullText load={async () => (await call<{ text: string }>('blob', { hash })).text} label={label} />;
}

/**
 * A short text whole; a long one by its first and last lines, with a button for the whole. It
 * never scrolls by itself, so the thread and the task panel are the only scrolling areas (#13).
 */
export function Output({ value }: { value: unknown }) {
  const blob = isBlob(value) ? value : null;
  const text = blob ? '' : typeof value === 'string' ? value : value == null ? '' : JSON.stringify(value, null, 2);
  if (!blob && !text) return null;
  if (!blob && fits(text)) {
    return (
      <div className="output">
        <pre>{text}</pre>
      </div>
    );
  }
  return (
    <div className="output">
      <pre>{firstLines(blob ? blob.head : text)}</pre>
      <div className="elided">
        …中间省略，共 {blob ? bytes(blob.size) : `${text.split('\n').length} 行`} ·{' '}
        {blob ? <BlobButton hash={blob.blob} /> : <FullText load={async () => text} />}
      </div>
      <pre>{lastLines(blob ? blob.tail : text)}</pre>
    </div>
  );
}
