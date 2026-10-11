// "共识" (manager-actions.md §3.3), read-only in M4: the goals, phases, decisions, constraints and
// open questions Angela recorded, by scope, each with the user's words it rests on. Only entries
// that still hold are listed. Her hypotheses are not shown here. Editing comes after M4.

import { useEffect } from 'react';
import type { LedgerEntry } from '../api/types';
import { clock, LEDGER_LABEL, ledgerHolds } from '../format';
import { useStore, watchLedger } from '../store';
import { Markdown } from './common';

export function Consensus() {
  const ledger = useStore((s) => s.ledger);
  useEffect(() => watchLedger(), []);
  if (!ledger) return null;
  const groups = [
    { key: 'user', name: '全局', entries: ledger.user.filter(ledgerHolds), unavailable: null },
    ...ledger.projects.map((p) => ({ key: p.id, name: p.name, entries: p.entries.filter(ledgerHolds), unavailable: p.unavailable })),
  ].filter((g) => g.entries.length > 0 || g.unavailable);
  return (
    <section className="overview consensus" aria-label="共识">
      <h2>共识</h2>
      {groups.length === 0 && <p className="muted">Angela 还没有记下目标、决定或约束。</p>}
      {groups.map((g) => (
        <section key={g.key} className="group" aria-label={g.name}>
          <h3>{g.name}</h3>
          {g.unavailable && <p className="muted">读不到：{g.unavailable}</p>}
          {g.entries.map((e) => (
            <Entry key={e.id} entry={e} />
          ))}
        </section>
      ))}
    </section>
  );
}

function Entry({ entry: e }: { entry: LedgerEntry }) {
  return (
    <div className="card ledger-entry">
      <p>
        <span className="badge">{LEDGER_LABEL[e.kind]}</span> {e.statement}
        {e.task_title && <span className="muted"> · 任务「{e.task_title}」</span>}
      </p>
      {e.evidence.map((q) => (
        <details key={q.source_id} className="ref-quote">
          <summary className="ref-head">
            你的原话 · {clock(q.said_at)} · {q.body.split('\n')[0].slice(0, 40)}
          </summary>
          <Markdown text={q.body} />
        </details>
      ))}
    </div>
  );
}
