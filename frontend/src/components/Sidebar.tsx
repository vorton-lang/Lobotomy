// The right column (frontend.md §2): what waits for the user, then the Workboard.

import { attentionKey } from '../format';
import { useStore } from '../store';
import { AttentionCard } from './Attention';
import { Workboard } from './Workboard';

export function Sidebar() {
  const attention = useStore((s) => s.snapshot?.attention ?? []);
  return (
    <aside className="sidebar">
      <section className="attention" aria-label="等你决定">
        <h3>
          等你决定 {attention.length > 0 && <span className="count">{attention.length}</span>}
        </h3>
        {attention.length === 0 && <p className="muted">现在没有需要你处理的事。</p>}
        {attention.map((a) => (
          <AttentionCard key={attentionKey(a)} attention={a} />
        ))}
      </section>
      <Workboard />
    </aside>
  );
}
