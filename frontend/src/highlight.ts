// Marks a search's matches in the mounted rows with the CSS Custom Highlight API (frontend.md
// §4.1), which needs no change to the rows' DOM. Rows carry `data-key`; the current match's row
// is marked apart. A match split across elements, such as half in bold, is not marked.

/**
 * Marks `query` in the rows under `root`. With `open` set to the current row's key, the collapsed
 * parts of that row that hold a match are opened, once. Returns whether that row was there.
 */
export function highlight(root: Element, query: string, current: string | null, open: string | null): boolean {
  const others: Range[] = [];
  const mine: Range[] = [];
  const needle = query.toLowerCase();
  if (!needle) {
    clearHighlight();
    return false;
  }
  let opened = false;
  // A turn's divider names its task; it is not searched, so it is not marked either.
  for (const row of root.querySelectorAll<HTMLElement>('[data-key]:not([data-key^="turn-"])')) {
    const isCurrent = row.dataset.key === current;
    if (isCurrent && open === current) {
      for (const details of row.querySelectorAll('details')) {
        if (details.textContent?.toLowerCase().includes(needle)) details.open = true;
      }
      opened = true;
    }
    const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.nodeValue?.toLowerCase() ?? '';
      for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + needle.length)) {
        const range = new Range();
        range.setStart(node, at);
        range.setEnd(node, at + needle.length);
        (isCurrent ? mine : others).push(range);
      }
    }
  }
  CSS.highlights.set('search', new Highlight(...others));
  CSS.highlights.set('search-current', new Highlight(...mine));
  return opened;
}

export function clearHighlight() {
  CSS.highlights.delete('search');
  CSS.highlights.delete('search-current');
}
