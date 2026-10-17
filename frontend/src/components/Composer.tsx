// Messages to Angela on 「全部」. A project's workers get messages through their tasks: the cards of
// "等你决定" and the task panel.

import { X } from 'lucide-react';
import { harnessName } from '../format';
import { act, clearDraft, setFollowing, setReplyTo, useCommand, useDraft, useMainRole, useStore } from '../store';
import { ICON } from './common';

export function Composer() {
  const [text, setDraft] = useDraft('angela');
  const role = useMainRole();
  const running = role?.unfinished?.state === 'running' ? role.unfinished : null;
  // What the message replies to, chosen on one of Angela's replies (manager-actions.md §1.1).
  const replyTo = useStore((s) => s.replyTo);

  const { submit, busy } = useCommand('send_message');
  if (!role) return null;
  const send = async () => {
    const body = text.trim();
    if (!body) return;
    const sent = await submit({ role: role.name, task_id: null, body, reply_to: replyTo?.id ?? null });
    if (sent === undefined) return;
    clearDraft('angela');
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
        onChange={(e) => setDraft(e.target.value)}
        placeholder={`发给 ${role.name}`}
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
          <button
            onClick={() => act('interrupt', { turn_id: running.id })}
            title={`像 Ctrl+C 一样请 ${harnessName(role.harness)} 停下`}
          >
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
