export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}

/**
 * Only links that cannot run code: the web, mail, and local files, which the desktop app opens
 * without running programs (frontend.md §7.2). A local file is a `file:` URL or an absolute path,
 * such as `C:\…`, `C:/…`, `/C:/…` (how Markdown keeps `</C:/…>`) or `/home/…`; it becomes a
 * `file:` URL. A relative path stays text: nothing says what it is relative to.
 */
export function safeHref(href: string): string | null {
  try {
    if (/^\/?[A-Za-z]:[\\/]/.test(href)) return new URL(`file:///${href.replace(/^\//, '').replaceAll('\\', '/')}`).href;
    if (/^\/[^/\\]/.test(href)) return new URL(`file://${href}`).href;
    const url = new URL(href);
    return ['http:', 'https:', 'mailto:', 'file:'].includes(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}
