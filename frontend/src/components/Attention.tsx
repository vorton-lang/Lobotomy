// The cards of "等你决定" (frontend.md §2): each says what waits for the user and offers the
// commands that resolve it. The backend decides what is listed (frontend.md §7).

import { useState } from 'react';
import type { Attention, Capture, Hold } from '../api/types';
import { abnormalEnd, bytes, harnessName, PERMISSION_LABEL, quotaNote, stoppedPaths } from '../format';
import { act, run, selectTask, setPermission, useCommand, useStore } from '../store';
import { Modal } from './common';
import { DiffPool, DiffView } from './DiffView';
import { NewTask } from './NewTask';

export function AttentionCard({ attention: a }: { attention: Attention }) {
  switch (a.kind) {
    case 'hold':
      return <HoldCard role={a.role} hold={a.hold} />;
    case 'outside_changes':
      return <OutsideChangesCard role={a.role} capture={a.capture} />;
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
          <p>「{a.title}」通过了验证，等你验收。</p>
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
    case 'job_failed':
      return (
        <div className="card warn">
          <p>运行时的工作出错：{a.key}</p>
          <p className="muted">{a.reason}</p>
          <div className="actions">
            <button onClick={() => act('retry_failed')}>重试</button>
          </div>
        </div>
      );
    case 'preview_stopped':
      return (
        <div className="card warn">
          <p>预览已停止：{a.reason}</p>
          <p className="muted">把仓库恢复到上次预览的状态后重试。Lobotomy 不会覆盖你的文件。</p>
          <div className="actions">
            <button onClick={() => run('retry_preview')}>重试预览</button>
          </div>
        </div>
      );
  }
}

/** Paths a card lists: the first dozen, then how many there are. */
function PathList({ lines, unit }: { lines: string[]; unit: string }) {
  return (
    <ul className="paths">
      {lines.slice(0, 12).map((line) => (
        <li key={line}>{line}</li>
      ))}
      {lines.length > 12 && (
        <li className="muted">
          ……共 {lines.length} {unit}
        </li>
      )}
    </ul>
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
        {failure?.kind === 'permission' && harness && (
          <p>
            这个环境的管理设置不允许 {name} 以「{PERMISSION_LABEL[harness.permission]}」运行。
            {switchable ? '可以改用自动审批：在沙箱里工作，超出沙箱的操作由自动审核决定。' : '可以在 ⚙ 设置里换一种权限模式，或请管理员调整。'}
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
  if (hold.kind === 'session_unidentified') {
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
  const c = hold.capture;
  const decide = (command: string) => run(command, { capture_id: c.id });
  const pending = c.kind === 'candidate' && '它报告的 done 还没有生效。';
  return (
    <div className="card warn">
      {c.state === 'oversized' ? (
        <p>
          {role} 的这次采集新增了 {c.detail?.files?.length ?? 0} 个文件，共 {bytes(c.detail?.total_bytes ?? 0)}，超过了项目设定的上限。
          {pending}
        </p>
      ) : (
        <p>
          {role} 的工作目录里有采集无法保存的内容。{pending}
        </p>
      )}
      <PathList lines={stoppedPaths(c)} unit="项" />
      <div className="actions">
        {c.state === 'oversized' && <button onClick={() => decide('approve_new_files')}>放行</button>}
        <button onClick={() => decide('discard_uncaptured')}>{c.state === 'oversized' ? '不要这些文件' : '跳过这些内容'}</button>
        <button className="primary" onClick={proceed} title="把清单交给执行者处理">
          继续
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

/** A turn outside any task changed files; nothing writes over them until the user decides (#14). */
function OutsideChangesCard({ role, capture }: { role: string; capture: Capture }) {
  const changed = capture.detail?.changed ?? [];
  const [viewing, setViewing] = useState(false);
  const [adopting, setAdopting] = useState(false);
  return (
    <div className="card warn">
      <p>
        {role} 在任务之外改了 {changed.length} 个文件。这些改动不属于任何任务，不会被验收或发布；你决定之前，不会开始下一个任务。
      </p>
      <PathList lines={changed} unit="个文件" />
      <div className="actions">
        <button onClick={() => setViewing(true)}>查看改动</button>
        <button onClick={() => run('discard_outside_changes', { capture_id: capture.id })} title="工作目录回到集成版本；采集记录仍保留这些改动">
          丢弃这些改动
        </button>
        <button className="primary" onClick={() => setAdopting(true)}>
          建成任务…
        </button>
      </div>
      {viewing && capture.commit_id && (
        <Modal title="任务之外的改动" onClose={() => setViewing(false)} wide>
          <DiffPool>
            <DiffView from={capture.base} to={capture.commit_id} />
          </DiffPool>
        </Modal>
      )}
      {adopting && <NewTask executor={role} fromCapture={capture.id} onClose={() => setAdopting(false)} />}
    </div>
  );
}

function BlockedCard({ taskId, title, reason }: { taskId: string; title: string; reason: string }) {
  const executor = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId)?.executor);
  return (
    <div className="card">
      <p>「{title}」的执行者在等你：</p>
      <p className="quote">{reason}</p>
      {executor && <Reply role={executor} taskId={taskId} />}
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
  placeholder = '回复',
  children,
}: {
  role: string;
  taskId: string;
  placeholder?: string;
  children?: React.ReactNode;
}) {
  const [reply, setReply] = useState('');
  const { submit, busy } = useCommand('send_message');
  return (
    <>
      <textarea value={reply} onChange={(e) => setReply(e.target.value)} rows={2} placeholder={placeholder} aria-label={placeholder} />
      <div className="actions">
        {children}
        <button
          className="primary"
          disabled={!reply.trim() || busy}
          onClick={async () => {
            const sent = await submit({ role, task_id: taskId, body: reply.trim() });
            if (sent !== undefined) setReply('');
          }}
        >
          发送
        </button>
      </div>
    </>
  );
}
