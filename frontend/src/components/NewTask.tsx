// A new task for a role.

import { clearDraft, selectTask, toast, useCommand, useDraft, useStore } from '../store';
import { Modal } from './common';

export function NewTask({ executor, onClose }: { executor: string; onClose: () => void }) {
  const project = useStore((s) => s.project);
  const keys = ['title', 'body', 'criteria'].map((field) => `new-task:${project}:${field}`);
  const [title, setTitle] = useDraft(keys[0]);
  const [body, setBody] = useDraft(keys[1]);
  const [criteria, setCriteria] = useDraft(keys[2]);
  const { submit, busy } = useCommand('create_task');
  // Closing keeps the draft for the next time; cancelling or creating the task drops it.
  const discard = () => {
    keys.forEach(clearDraft);
    onClose();
  };
  const create = async () => {
    const created = await submit({ title, body, criteria, executor });
    if (!created) return;
    discard();
    // An open task panel would cover the thread where the new task's work shows (#13); the
    // earlier task stays a click away in the Workboard.
    selectTask(null);
    toast(`「${title}」已交给 ${executor}`);
  };
  return (
    <Modal title="新任务" onClose={onClose}>
      <label>
        标题
        <input value={title} onChange={(e) => setTitle(e.target.value)} autoFocus />
      </label>
      <label>
        你的原话
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          rows={6}
          placeholder="要做什么、为什么、有什么要注意的"
        />
      </label>
      <label>
        完成条件
        <textarea
          value={criteria}
          onChange={(e) => setCriteria(e.target.value)}
          rows={3}
          placeholder="怎样算完成，最好能被检查"
        />
      </label>
      <div className="actions">
        <button onClick={discard}>取消</button>
        <button className="primary" disabled={!title.trim() || busy} onClick={create}>
          交给 {executor}
        </button>
      </div>
    </Modal>
  );
}
