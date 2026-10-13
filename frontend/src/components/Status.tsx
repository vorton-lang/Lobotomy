// Who, and in what state (ui-ux-plan.md P1). Each role has a color and an initial of its own; a
// state shows in one of five looks, each with its own shape, so color is never the only sign.

import type { ReactNode } from 'react';
import { Check, TriangleAlert } from 'lucide-react';
import { quietFor, roleHue, toolRunning, type Look } from '../format';
import { lastOutputAt, useStore } from '../store';
import { useNow } from './common';

export function RoleAvatar({ name }: { name: string }) {
  return (
    <span className={`avatar role-${roleHue(name)}`} aria-hidden>
      {[...name][0]?.toUpperCase()}
    </span>
  );
}

export function RoleName({ name }: { name: string }) {
  return <span className={`role-name role-${roleHue(name)}`}>{name}</span>;
}

/** Who wrote a message: the role's avatar and name. */
export function Author({ name }: { name: string }) {
  return (
    <>
      <RoleAvatar name={name} />
      <RoleName name={name} />
    </>
  );
}

function StatusMark({ look }: { look: Look }) {
  if (look === 'fail') return <TriangleAlert className="mark" size={13} strokeWidth={2.5} aria-hidden />;
  if (look === 'done') return <Check className="mark" size={13} strokeWidth={3} aria-hidden />;
  return <span className="mark" aria-hidden />;
}

/** A short state, such as a task's phase, as a pill. */
export function StatusPill({
  look,
  className = '',
  children,
}: {
  look: Look;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span className={`pill ${look} ${className}`}>
      <StatusMark look={look} />
      {children}
    </span>
  );
}

/** A state said in a sentence, such as what a role does: the mark and the words. */
export function StatusText({ look, children }: { look: Look; children: ReactNode }) {
  return (
    <span className={`status-text ${look}`}>
      <StatusMark look={look} />
      <span>{children}</span>
    </span>
  );
}

/** A running turn that has shown nothing new for a while says so (ui-ux-plan.md §8 第 1 条). */
export function useQuiet(turnId: string | undefined): string | null {
  const live = useStore((s) => (turnId ? s.live[turnId] : undefined));
  const now = useNow(live !== undefined);
  if (!turnId || !live || toolRunning(live)) return null;
  return quietFor(lastOutputAt(turnId), now);
}

export function QuietNote({ turnId }: { turnId: string }) {
  const quiet = useQuiet(turnId);
  return quiet ? <div className="quiet-note">{quiet}</div> : null;
}
