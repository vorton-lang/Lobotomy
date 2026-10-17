// 「全部」 (frontend.md §2): what waits for the user in every project, grouped by
// project. An item is acted on in its project: 「前往」 shows the project and opens the task the
// item is about. Blocked quota domains belong to no project; they are listed once under 「本机」 and
// retried here (data-model.md §10.5). Angela's own items, such as a turn to continue, are acted on
// here: her snapshot is the window's. With Angela open this list is the right column beside her
// conversation (M4); without her it fills 「全部」, with the reason she is not open. A
// `workers.toml` that could not be read is said first, here and in each project.

import { attentionKey, attentionSummary, attentionTask } from '../format';
import { angelaOpen, selectProject, totalAttention, useStore } from '../store';
import { AttentionCard } from './Attention';

export function Overview() {
  const host = useStore((s) => s.host);
  if (!host) return null;
  const total = totalAttention(host);
  const projects = host.projects.filter((p) => p.attention.length > 0);
  const angela = host.angela;
  return (
    <section className="overview" aria-label="全部项目">
      {angela?.error && <p className="error">Angela 没有打开：{angela.error}</p>}
      <WorkersError />
      <h2>等你决定 {total > 0 && <span className="count">{total}</span>}</h2>
      {total === 0 && <p className="muted">现在没有需要你处理的事。</p>}
      {angelaOpen(host) && angela.attention.length > 0 && (
        <section className="group" aria-label="Angela">
          <h3>Angela</h3>
          {angela.attention.map((a) => (
            <AttentionCard key={attentionKey(a)} attention={a} />
          ))}
        </section>
      )}
      {host.attention.length > 0 && (
        <section className="group" aria-label="本机">
          <h3>本机</h3>
          {host.attention.map((a) => (
            <AttentionCard key={attentionKey(a)} attention={a} />
          ))}
        </section>
      )}
      {projects.map((p) => (
        <section key={p.id} className="group" aria-label={p.name}>
          <h3>{p.name}</h3>
          {p.attention.map((a) => (
            <div key={attentionKey(a)} className="card">
              <p>{attentionSummary(a)}</p>
              <div className="actions">
                <button onClick={() => selectProject(p.id, attentionTask(a))}>前往</button>
              </div>
            </div>
          ))}
        </section>
      ))}
    </section>
  );
}

/** Why no worker runs: `workers.toml` could not be read. Shown in 「全部」 and in every project. */
export function WorkersError() {
  const error = useStore((s) => s.host?.workers_error);
  if (!error) return null;
  return (
    <div className="card warn">
      <p>workers.toml 读不出来，所有 worker 都没有启动。改好后重启 Lobotomy。</p>
      <p className="muted failure">{error}</p>
    </div>
  );
}
