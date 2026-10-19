// The cards of "等你决定" (frontend.md §2): each says what waits for the user and offers the
// commands that resolve it. The backend decides what is listed (frontend.md §7).

import type { Attention, Hold, QuestionKind, TaskView } from '../api/types';
import { abnormalEnd, harnessName, PERMISSION_LABEL, quotaNote } from '../format';
import { act, clearDraft, run, selectTask, setPermission, useCommand, useDraft, useStore } from '../store';

export function AttentionCard({ attention: a }: { attention: Attention }) {
  switch (a.kind) {
    case 'hold':
      return <HoldCard role={a.role} hold={a.hold} />;
    case 'unknown_turn':
      return (
        <div className="card warn">
          <p>后端重启前启动的 {a.role} 的 CLI 仍在运行。运行时不会自动结束它。</p>
          <div className="actions">
            <button onClick={() => act('terminate', { turn_id: a.turn_id })}>终止残留进程</button>
          </div>
        </div>
      );
    case 'task_blocked':
      return <BlockedCard taskId={a.task_id} title={a.title} reason={a.reason} />;
    case 'stalled':
      return <StalledCard attention={a} />;
    case 'accept':
      return (
        <div className="card">
          <p>「{a.title}」交出了候选成果，等你验收。</p>
          <UndeliveredNote taskId={a.task_id} />
          <div className="actions">
            <button className="primary" onClick={() => selectTask(a.task_id)}>
              查看并验收
            </button>
          </div>
        </div>
      );
    case 'quota':
      return (
        <div className="card warn">
          <p>{quotaNote(a.domain)}</p>
          {a.domain.message && <p className="muted">{a.domain.message}</p>}
          <div className="actions">
            <button onClick={() => act('quota_retry', { harness: a.domain.harness })}>额度重试</button>
          </div>
        </div>
      );
    case 'question':
      return <QuestionCard attention={a} />;
  }
}

const ASKS: Record<QuestionKind, string> = { confirm: '请你确认', choice: '请你选择', open: '问你' };

/**
 * Angela's question (tech-plan.md §5 "要你卡"). An option goes to her as the user's answer to this
 * question; the user can always write one instead.
 * A later user message to her without a question ID is also a candidate answer.
 */
function QuestionCard({ attention: a }: { attention: Extract<Attention, { kind: 'question' }> }) {
  const { submit, busy } = useCommand('send_message');
  return (
    <div className="card">
      <p>Angela {ASKS[a.question_kind]}：</p>
      <p className="quote">{a.question}</p>
      {a.options.length > 0 && (
        <div className="actions">
          {a.options.map((option) => (
            <button
              key={option}
              disabled={busy}
              onClick={() => submit({ role: 'Angela', task_id: null, body: option, question_id: a.id })}
            >
              {option}
            </button>
          ))}
        </div>
      )}
      <Reply
        role="Angela"
        taskId={null}
        questionId={a.id}
        placeholder={a.options.length > 0 ? '或者自己写' : '回复 Angela'}
      />
    </div>
  );
}

function HoldCard({ role, hold }: { role: string; hold: Hold }) {
  const roleView = useStore((s) => s.snapshot?.roles.find((r) => r.name === role));
  const harness = useStore((s) => s.snapshot?.harnesses.find((h) => h.harness === roleView?.harness));
  const name = harnessName(roleView?.harness ?? '');
  const taskId = roleView?.task_id;
  const proceed = () => run('continue', { role });
  const newSession = () => run('new_native_session', { role });
  const abandon = taskId ? () => run('abandon', { task_id: taskId, reason: '用户在异常后放弃' }) : undefined;
  if (hold.kind === 'abnormal') {
    const failure = hold.turn.failure;
    // The environment refused full access: the one-step way on is auto review (harness-adapter.md §1.9).
    const switchable = failure?.kind === 'permission' && harness?.permission === 'full';
    const switchAndProceed = async () => {
      if (!harness || (await setPermission(harness.harness, 'auto_review')) === undefined) return;
      await proceed();
    };
    return (
      <div className="card warn">
        <p>
          {role} 的上一个 turn {abnormalEnd(hold.turn)}，它停下等你决定。
        </p>
        {/* Only the current setting is known: it may have changed since the turn failed, and
            continuing starts the CLI with it. */}
        {failure?.kind === 'permission' && harness && (
          <p>
            {switchable
              ? `这个环境的管理设置不允许 ${name} 以「完全放开」运行。可以改用自动审批：在沙箱里工作，超出沙箱的操作由自动审核决定。`
              : `这个环境的管理设置不允许 ${name} 以当时的权限模式运行。现在的设置是「${PERMISSION_LABEL[harness.permission]}」，继续时按它重新启动；仍被拒绝时，请管理员调整。`}
          </p>
        )}
        {failure?.unstarted && <p className="muted">{name} 没有收到那次的内容；继续时会原样重新发送。</p>}
        {failure?.kind === 'permission' ? (
          <details>
            <summary className="muted">原始错误</summary>
            <p className="muted failure">{failure.message}</p>
          </details>
        ) : (
          failure && <p className="muted failure">{failure.message}</p>
        )}
        <div className="actions">
          {switchable && (
            <button className="primary" onClick={switchAndProceed}>
              改用自动审批并继续
            </button>
          )}
          <button className={switchable ? '' : 'primary'} onClick={proceed}>
            继续
          </button>
          <button onClick={newSession}>新会话</button>
          {abandon && <button onClick={abandon}>放弃任务</button>}
        </div>
      </div>
    );
  }
  return (
    <div className="card warn">
      <p>
        {role} 的会话没有记下 {name} 的会话 ID，无法接着上次继续。
      </p>
      <div className="actions">
        <button className="primary" onClick={newSession}>
          新会话
        </button>
      </div>
    </div>
  );
}

function UndeliveredNote({ taskId }: { taskId: string }) {
  const count = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId)?.undelivered_messages ?? 0);
  if (count === 0) return null;
  return <p className="muted">还有 {count} 条你发的消息没交给执行者，验收前要先决定怎么处理。</p>;
}

function BlockedCard({ taskId, title, reason }: { taskId: string; title: string; reason: string }) {
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId));
  // A stopped worker goes on from a reply to this stop, and only from the first one
  // (worker-interface.md §3).
  const stopId = task?.worker?.status.blocked?.stop_id ?? null;
  return (
    <div className="card">
      <p>「{title}」的执行者在等你：</p>
      <p className="quote">{reason}</p>
      {task && <Reply role={task.executor} taskId={taskId} stopId={stopId} />}
    </div>
  );
}

/**
 * Replies being written when their card went, as when Angela continued the worker first: they are
 * not dropped silently (worker-interface.md §3). One can still go as a plain message, which waits
 * for the worker's next turn.
 */
export function LeftoverReplies({ attention }: { attention: Attention[] }) {
  const tasks = useStore((s) => s.snapshot?.tasks);
  const drafts = useStore((s) => s.drafts);
  const shown = new Set(attention.map((a) => (a.kind === 'task_blocked' || a.kind === 'stalled' ? a.task_id : null)));
  const left = (tasks ?? []).filter(
    (t) => ['executing', 'accepting'].includes(t.phase) && drafts[`reply:${t.id}`]?.trim() && !shown.has(t.id),
  );
  return left.map((t) => <LeftoverReply key={t.id} task={t} />);
}

function LeftoverReply({ task }: { task: TaskView }) {
  const key = `reply:${task.id}`;
  const text = useStore((s) => s.drafts[key] ?? '');
  const { submit, busy } = useCommand('send_message');
  const send = async () => {
    const sent = await submit({ role: task.executor, task_id: task.id, body: text.trim() });
    if (sent !== undefined) clearDraft(key);
  };
  return (
    <div className="card">
      <p>「{task.title}」的卡片已经处理掉了，你写到一半的回复没有发出：</p>
      <p className="quote">{text}</p>
      <div className="actions">
        <button onClick={() => clearDraft(key)}>丢弃</button>
        <button className="primary" disabled={busy} onClick={send}>
          作为普通消息发送
        </button>
      </div>
    </div>
  );
}

/** The turn ended normally, the task did not: nothing moves it on but the user (#13). */
function StalledCard({ attention: a }: { attention: Extract<Attention, { kind: 'stalled' }> }) {
  return (
    <div className="card">
      <p>
        {a.role} 的 turn 已经结束，但「{a.title}」还没有完成：它没有报告完成，也没有提出问题。
      </p>
      {a.report_error && <p className="muted failure">它调用 org_report 没有成功：{a.report_error}</p>}
      <p className="muted">看看它最后说了什么，再告诉它接下来做什么。</p>
      <Reply role={a.role} taskId={a.task_id} placeholder={`发给 ${a.role}`}>
        <button onClick={() => selectTask(a.task_id)}>查看任务</button>
      </Reply>
    </div>
  );
}

function Reply({
  role,
  taskId,
  stopId = null,
  questionId = null,
  placeholder = '回复',
  children,
}: {
  role: string;
  taskId: string | null;
  /** The worker's stop the reply continues. */
  stopId?: string | null;
  /** Angela's question the reply answers. */
  questionId?: string | null;
  placeholder?: string;
  children?: React.ReactNode;
}) {
  // A task's id names its reply, as Angela's question's does.
  const key = `reply:${taskId ?? questionId ?? role}`;
  const [reply, setReply] = useDraft(key);
  const { submit, busy } = useCommand('send_message');
  return (
    <>
      <textarea
        value={reply}
        onChange={(e) => setReply(e.target.value)}
        rows={2}
        placeholder={placeholder}
        aria-label={placeholder}
      />
      <div className="actions">
        {children}
        <button
          className="primary"
          disabled={!reply.trim() || busy}
          onClick={async () => {
            const sent = await submit({
              role,
              task_id: taskId,
              body: reply.trim(),
              stop_id: stopId,
              question_id: questionId,
            });
            if (sent !== undefined) clearDraft(key);
          }}
        >
          发送
        </button>
      </div>
    </>
  );
}
