// Messages to Malkuth. While it works on a task, a message belongs to that task. Outside
// execution, a message waits for the task to return to execution; closing the task does not
// deliver it, and accepting needs the user to let it go (data-model.md §4.2, #14, #15).

import { useState } from 'react';
import { act, MAIN_ROLE, useCommand, useStore } from '../store';

export function Composer() {
  const [text, setText] = useState('');
  const role = useStore((s) => s.snapshot?.roles.find((r) => r.name === MAIN_ROLE));
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === role?.task_id));
  const running = role?.unfinished?.state === 'running' ? role.unfinished : null;

  const { submit, busy } = useCommand('send_message');
  const send = async () => {
    const body = text.trim();
    if (!body) return;
    const sent = await submit({ role: MAIN_ROLE, task_id: task?.id ?? null, body });
    if (sent !== undefined) setText('');
  };

  const hint = !task
    ? '发给 Malkuth（不属于任何任务）'
    : task.phase === 'executing'
      ? `发给 Malkuth · 任务「${task.title}」`
      : task.phase === 'verifying'
        ? `任务「${task.title}」正在验证。消息先排队，任务回到执行时交给 Malkuth；验收前要你决定退回还是不再投递`
        : `任务「${task.title}」等你验收。消息先排队，退回后交给 Malkuth；直接验收则不再投递`;

  return (
    <div className="composer">
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={hint}
        aria-label="消息"
        rows={3}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            void send();
          }
        }}
      />
      <div className="composer-actions">
        {running && (
          <button onClick={() => act('interrupt', { turn_id: running.id })} title="像 Ctrl+C 一样请 Codex 停下">
            中断
          </button>
        )}
        <button className="primary" onClick={send} disabled={!text.trim() || busy}>
          发送
        </button>
      </div>
    </div>
  );
}
