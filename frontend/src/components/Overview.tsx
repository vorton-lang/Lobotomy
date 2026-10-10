// 「全部」 (frontend.md §2 "M3 的布局"): what waits for the user in every project, grouped by
// project. An item is acted on in its project: 「前往」 shows the project and opens the task the
// item is about. Blocked quota domains belong to no project; they are listed once under 「本机」 and
// retried here (data-model.md §10.5). In M4 this is the conversation with Angela.

import { attentionKey, attentionSummary, attentionTask } from '../format';
import { selectProject, totalAttention, useStore } from '../store';
import { AttentionCard } from './Attention';

export function Overview() {
  const host = useStore((s) => s.host);
  if (!host) return null;
  const total = totalAttention(host);
  const projects = host.projects.filter((p) => p.attention.length > 0);
  return (
    <section className="overview" aria-label="全部项目">
      <h2>
        等你决定 {total > 0 && <span className="count">{total}</span>}
      </h2>
      {total === 0 && <p className="muted">现在没有需要你处理的事。</p>}
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
