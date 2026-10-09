// A new task for a role. Made from changes outside any task, it starts from them (#14).

import { useState } from 'react';
import { selectTask, toast, useCommand } from '../store';
import { Modal } from './common';

export function NewTask({ executor, fromCapture, onClose }: { executor: string; fromCapture?: string; onClose: () => void }) {
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [criteria, setCriteria] = useState('');
  const { submit, busy } = useCommand<{ id: string }>(fromCapture ? 'adopt_outside_changes' : 'create_task');
  const create = async () => {
    // A task made from changes goes to the role whose slot holds them.
    const created = await submit(fromCapture ? { capture_id: fromCapture, title, body, criteria } : { title, body, criteria, executor });
    if (!created) return;
    onClose();
    // An open task panel would cover the thread where the new task's work shows (#13); the
    // earlier task stays a click away in the Workboard.
    selectTask(null);
    toast(`「${title}」已交给 ${executor}`);
  };
  return (
    <Modal title={fromCapture ? '用这些改动建任务' : '新任务'} onClose={onClose}>
      {fromCapture && <p className="muted">任务从这些改动开始：执行者在它们的基础上继续，做完后照常验证、验收。</p>}
      <label>
        标题
        <input value={title} onChange={(e) => setTitle(e.target.value)} autoFocus />
      </label>
      <label>
        你的原话
        <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={6} placeholder="要做什么、为什么、有什么要注意的" />
      </label>
      <label>
        完成条件
        <textarea value={criteria} onChange={(e) => setCriteria(e.target.value)} rows={3} placeholder="怎样算完成，最好能被检查" />
      </label>
      <div className="actions">
        <button onClick={onClose}>取消</button>
        <button className="primary" disabled={!title.trim() || busy} onClick={create}>
          交给 {executor}
        </button>
      </div>
    </Modal>
  );
}
