// Dragging a column's edge to change its width (frontend.md §2): the project list, the right
// column and the task panel. The width is a convenience of this viewer, kept in localStorage; a
// storage that cannot be read or written leaves the default. The handle also takes the arrow keys,
// and a double click goes back to the default width.

import { useCallback, useRef, useState } from 'react';

const STEP = 16;

function stored(key: string): number | null {
  try {
    const value = Number(localStorage.getItem(`lobotomy.width.${key}`));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

function store(key: string, width: number) {
  try {
    localStorage.setItem(`lobotomy.width.${key}`, String(Math.round(width)));
  } catch {
    // The width stays for this session only.
  }
}

/** A column's width in pixels, between `min` and `max`, remembered under `key`. */
export function useWidth(key: string, initial: number, min: number, max: number) {
  const clamp = useCallback((w: number) => Math.min(max, Math.max(min, w)), [min, max]);
  const [width, setWidth] = useState(() => clamp(stored(key) ?? initial));
  /** Sets the width; `keep` also remembers it, which a drag does once it ends. */
  const set = useCallback(
    (w: number, keep = true) => {
      const next = clamp(w);
      setWidth(next);
      if (keep) store(key, next);
    },
    [clamp, key],
  );
  return { width, set, reset: () => set(initial), min, max };
}

/**
 * The handle on a column's edge. `edge` is the side of the column it sits on: dragging away from
 * the column widens it.
 */
export function Sash({ label, edge, column }: { label: string; edge: 'left' | 'right'; column: ReturnType<typeof useWidth> }) {
  const drag = useRef<{ x: number; width: number } | null>(null);
  const sign = edge === 'right' ? 1 : -1;
  return (
    <div
      className={`sash ${edge}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={column.width}
      aria-valuemin={column.min}
      aria-valuemax={column.max}
      tabIndex={0}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { x: e.clientX, width: column.width };
      }}
      onPointerMove={(e) => {
        if (drag.current) column.set(drag.current.width + sign * (e.clientX - drag.current.x), false);
      }}
      onPointerUp={() => {
        drag.current = null;
        column.set(column.width);
      }}
      onPointerCancel={() => {
        drag.current = null;
        column.set(column.width);
      }}
      onDoubleClick={column.reset}
      onKeyDown={(e) => {
        if (e.key === 'ArrowLeft') column.set(column.width - sign * STEP);
        else if (e.key === 'ArrowRight') column.set(column.width + sign * STEP);
        else return;
        e.preventDefault();
      }}
    />
  );
}
