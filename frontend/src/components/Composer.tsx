// Messages to Malkuth. While it works on a task, a message belongs to that task. Outside
// execution, messages wait until the task returns or closes (data-model.md §4.2).

import { useState } from 'react';
import { act, MAIN_ROLE, run, useStore } from '../store';

export function Composer() {
  const [text, setText] = useState('');
  const role = useStore((s) => s.snapshot?.roles.find((r) => r.name === MAIN_ROLE));
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === role?.task_id));
  const running = role?.unfinished?.state === 'running' ? role.unfinished : null;

  const send = async () => {
    const body = text.trim();
    if (!body) return;
    const sent = await run('send_message', { role: MAIN_ROLE, task_id: task?.id ?? null, body });
    if (sent !== undefined) setText('');
  };

  const hint = task
    ? task.phase === 'executing'
      ? `发给 Malkuth · 任务「${task.title}」`
      : `任务「${task.title}」在${task.phase === 'verifying' ? '验证' : '验收'}阶段，消息会在任务回到执行或关闭后投递`
    : '发给 Malkuth（不属于任何任务）';

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
        <button className="primary" onClick={send} disabled={!text.trim()}>
          发送
        </button>
      </div>
    </div>
  );
}
