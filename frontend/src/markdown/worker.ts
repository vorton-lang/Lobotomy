// Markdown to HTML off the main thread (frontend.md §4.1). Agent text is untrusted: raw HTML is
// shown as text, links only open http(s) and mailto, images are not loaded.

import { Marked, type Tokens } from 'marked';
import remend from 'remend';
import { createHighlighter, type Highlighter } from 'shiki';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import { escapeHtml, safeHref } from './escape';

/** Code longer than this is shown without highlighting. */
const HIGHLIGHT_LIMIT = 50_000;

let highlighter: Promise<Highlighter> | null = null;
function getHighlighter() {
  highlighter ??= createHighlighter({
    themes: ['github-light', 'github-dark'],
    langs: [],
    engine: createJavaScriptRegexEngine(),
  });
  return highlighter;
}

async function highlight(code: string, lang: string | undefined): Promise<string | null> {
  if (!lang || code.length > HIGHLIGHT_LIMIT) return null;
  const h = await getHighlighter();
  try {
    if (!h.getLoadedLanguages().includes(lang)) await h.loadLanguage(lang as never);
    return h.codeToHtml(code, { lang, themes: { light: 'github-light', dark: 'github-dark' }, defaultColor: false });
  } catch {
    return null;
  }
}

type CodeToken = Tokens.Code & { highlighted?: string | null };

const marked = new Marked({
  async: true,
  gfm: true,
  breaks: true,
  async walkTokens(token) {
    if (token.type === 'code') {
      const code = token as CodeToken;
      code.highlighted = await highlight(code.text, code.lang?.split(/\s/)[0]);
    }
  },
  renderer: {
    html: ({ text }) => escapeHtml(text),
    image: ({ text }) => escapeHtml(text),
    link({ href, tokens }) {
      const inner = this.parser.parseInline(tokens);
      const safe = safeHref(href);
      return safe ? `<a href="${escapeHtml(safe)}" target="_blank" rel="noreferrer">${inner}</a>` : inner;
    },
    code(token) {
      const code = token as CodeToken;
      return code.highlighted ?? `<pre class="code"><code>${escapeHtml(code.text)}</code></pre>`;
    },
  },
});

/**
 * Repairs only what marked would read differently once complete. Math is not rendered, so `$$`
 * is left alone; single tildes and `- >` keep the meaning marked gives them in finished text.
 */
const REPAIR = { katex: false, singleTilde: false, comparisonOperators: false } as const;

const FENCE = /^ {0,3}(`{3,}|~{3,})/;

/**
 * Where the last block starts. Only it can still be unfinished: marked does not pair emphasis
 * across a blank line, so repairing earlier blocks would add marks no block closes. A fence still
 * open at the end makes the last block start at its opening line.
 */
export function lastBlockStart(text: string): number {
  let start = 0;
  let fence: string | null = null;
  let offset = 0;
  for (const line of text.split('\n')) {
    const marker = FENCE.exec(line)?.[1];
    const next = offset + line.length + 1;
    if (fence === null) {
      if (marker) {
        fence = marker;
        start = offset;
      } else if (line.trim() === '') {
        start = next;
      }
    } else if (marker && marker[0] === fence[0] && marker.length >= fence.length && line.trim() === marker) {
      fence = null;
      start = next;
    }
    offset = next;
  }
  return Math.min(start, text.length);
}

function repair(text: string): string {
  const start = lastBlockStart(text);
  return text.slice(0, start) + remend(text.slice(start), REPAIR);
}

self.onmessage = async (event: MessageEvent<{ id: number; text: string; streaming: boolean }>) => {
  const { id, text, streaming } = event.data;
  try {
    const source = streaming ? repair(text) : text;
    // Unrepaired live text renders exactly as the completed item will.
    self.postMessage({ id, html: await marked.parse(source), repaired: source !== text });
  } catch (e) {
    self.postMessage({ id, html: `<pre>${escapeHtml(text)}</pre>`, error: String(e) });
  }
};
