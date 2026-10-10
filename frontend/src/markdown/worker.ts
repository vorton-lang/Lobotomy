// Markdown to HTML off the main thread (frontend.md §4.1). Agent text is untrusted: raw HTML is
// shown as text, links only open http(s) and mailto, images are not loaded.

import { Marked, type Tokens } from 'marked';
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

self.onmessage = async (event: MessageEvent<{ id: number; text: string }>) => {
  const { id, text } = event.data;
  try {
    const html = await marked.parse(text);
    self.postMessage({ id, html });
  } catch (e) {
    self.postMessage({ id, html: `<pre>${escapeHtml(text)}</pre>`, error: String(e) });
  }
};
