// The design tokens (notes/ui-ux-plan.md §3): colors live only in the light and dark :root blocks,
// and the pairs the rules put together are readable in both (WCAG AA: 4.5:1 for text, 3:1 for a
// focus ring or a mark that carries meaning).

import { describe, expect, it } from 'vitest';
import source from './styles.css?raw';

const css = source.replace(/\/\*[\s\S]*?\*\//g, '');
const light = css.match(/^:root \{([^}]*)\}/m)![1];
const dark = css.match(/@media \(prefers-color-scheme: dark\) \{\s*:root \{([^}]*)\}/)![1];

function tokens(block: string): Record<string, string> {
  return Object.fromEntries([...block.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const TEXT: [string, string[]][] = [
  ['text', ['bg', 'bg-rail', 'panel', 'sunken', 'hover', 'active', 'mine', 'wait-soft', 'accent-soft']],
  ['muted', ['bg', 'bg-rail', 'panel', 'sunken', 'hover', 'mine']],
  ['accent-text', ['bg', 'bg-rail', 'panel']],
  ['on-accent', ['accent', 'accent-hover']],
  ['on-wait', ['wait-solid']],
  ['run', ['bg', 'bg-rail', 'panel']],
  ['wait', ['bg', 'panel']],
  ['fail', ['bg', 'bg-rail', 'panel', 'fail-soft']],
  ['done', ['bg', 'bg-rail', 'panel', 'done-soft']],
];
const MARKS: [string, string[]][] = [
  ['focus', ['bg', 'bg-rail', 'panel']],
  ['run-solid', ['bg', 'bg-rail', 'panel']],
  ['fail-solid', ['bg', 'bg-rail', 'panel']],
  ['done-solid', ['bg', 'bg-rail', 'panel']],
];

describe('the design tokens', () => {
  it('keep colors out of the rules', () => {
    const rules = css.replace(light, '').replace(dark, '').replace(/url\("[^"]*"\)/g, '');
    expect(rules.match(/#[0-9a-f]{3,8}\b|\b(rgba?|hsla?|oklch|lab|lch)\(/gi) ?? []).toEqual([]);
  });

  it('give every color a dark value', () => {
    const colors = Object.entries(tokens(light)).filter(([, v]) => /^(#|rgb|0 )/.test(v)).map(([k]) => k);
    const missing = colors.filter((k) => !(k in tokens(dark)));
    expect(missing).toEqual([]);
  });

  for (const [name, block] of [['light', light], ['dark', dark]] as const) {
    const t = tokens(block);
    it(`read in ${name}`, () => {
      const failing: string[] = [];
      for (const [pairs, min] of [[TEXT, 4.5], [MARKS, 3]] as const) {
        for (const [fg, backs] of pairs) {
          for (const back of backs) {
            const ratio = contrast(t[fg], t[back]);
            if (ratio < min) failing.push(`${fg} on ${back}: ${ratio.toFixed(2)}`);
          }
        }
      }
      expect(failing).toEqual([]);
    });
  }
});
