/**
 * Search helpers shared by the global search page and in-session search.
 */

export interface SnippetSegment {
  text: string;
  /** Whether the segment is a matched term (the server wraps it in `[ ]`). */
  match: boolean;
}

/**
 * Split an FTS5 snippet into plain and matched segments.
 *
 * The server asks SQLite for `snippet(..., '[', ']', ...)`, so a hit looks like
 * `before [term] after`. Unbalanced brackets are treated as plain text.
 */
export function snippetSegments(snippet: string): SnippetSegment[] {
  const parts: SnippetSegment[] = [];
  let rest = snippet;
  while (rest !== "") {
    const start = rest.indexOf("[");
    const end = start >= 0 ? rest.indexOf("]", start + 1) : -1;
    if (start < 0 || end < 0) {
      parts.push({ text: rest, match: false });
      break;
    }
    if (start > 0) parts.push({ text: rest.slice(0, start), match: false });
    parts.push({ text: rest.slice(start + 1, end), match: true });
    rest = rest.slice(end + 1);
  }
  return parts;
}
