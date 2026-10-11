// Messages to the main role. While it works on a task, a message belongs to that task; the hint
// says where it goes (`composerHint`).

import { useState } from 'react';
import { X } from 'lucide-react';
import { composerHint, harnessName } from '../format';
import { act, setFollowing, setReplyTo, useCommand, useMainRole, useStore } from '../store';
import { ICON } from './common';

export function Composer() {
  const [text, setText] = useState('');
  const role = useMainRole();
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === role?.task_id));
  const running = role?.unfinished?.state === 'running' ? role.unfinished : null;
  // What the message replies to, chosen on one of Angela's replies (manager-actions.md §1.1).
  const replyTo = useStore((s) => (role?.kind === 'manager' ? s.replyTo : null));

  const { submit, busy } = useCommand('send_message');
  if (!role) return null;
  const send = async () => {
    const body = text.trim();
    if (!body) return;
    const sent = await submit({ role: role.name, task_id: task?.id ?? null, body, reply_to: replyTo?.id ?? null });
    if (sent === undefined) return;
    setText('');
    setReplyTo(null);
    // What the user just sent, and what comes of it, is at the end of the thread.
    setFollowing(true);
  };

  return (
    <div className="composer">
      {replyTo && (
        <div className="replying">
          <span>
            回复 {role.name}：「{replyTo.text}」
          </span>
          <button className="icon" aria-label="不回复这一条" onClick={() => setReplyTo(null)}>
            <X {...ICON} />
          </button>
        </div>
      )}
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={composerHint(role.name, task)}
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
          <button onClick={() => act('interrupt', { turn_id: running.id })} title={`像 Ctrl+C 一样请 ${harnessName(role.harness)} 停下`}>
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
