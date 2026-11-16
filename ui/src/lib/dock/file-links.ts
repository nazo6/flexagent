/**
 * Resolution of chat links that point at a workspace file.
 *
 * Agent output commonly links to files with a relative path (`src/a.ts`), an
 * absolute path, a `file:` URI, and an optional line anchor (`#L12` / `:12`).
 * The browser cannot open node paths, so the chat renderer resolves them here
 * and hands them to the Files dock. Anything that is not a workspace file (web
 * links, fragments, directories) returns `null` and keeps its normal behavior.
 */

import { isAbsolutePath, isInsidePath, joinPath, normalizePath } from "./paths.ts";

/** One resolved file link. */
export interface FileLinkTarget {
  /** Absolute node path. */
  path: string;
  /** 1-based line to focus, when the link carries one. */
  line: number | null;
}

/** Schemes that are URLs, not node paths. */
const IGNORED_SCHEME =
  /^(?:https?|mailto|tel|callto|sms|cid|xmpp|matrix|data|javascript|vbscript):/i;
const FILE_SCHEME = /^file:/i;
const SCHEME = /^[a-z][a-z+.-]*:/i;
const DRIVE_PATH = /^[A-Za-z]:(?:[\\/]|%5c|%2f)/i;

/** `#L12`, `#L12-L20`, `#12` */
function lineFromFragment(fragment: string): number | null {
  const match = /^L?(\d+)(?:-L?\d+)?$/i.exec(fragment);
  return match === null ? null : Number(match[1]);
}

/** `src/a.ts:12`, `src/a.ts:12:5` (not the drive colon of `C:\...`). */
function splitLineSuffix(path: string): { path: string; line: number | null } {
  const match = /^(.*?):(\d+)(?::\d+)?$/.exec(path);
  if (match === null || /^[A-Za-z]:$/.test(match[1])) {
    return { path, line: null };
  }
  return { path: match[1], line: Number(match[2]) };
}

function decode(path: string): string {
  try {
    return decodeURIComponent(path);
  } catch {
    return path;
  }
}

/** Resolve a `file:` URI (local, Windows drive, or UNC share) into a node path. */
function fromFileUrl(href: string): FileLinkTarget | null {
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  if (!FILE_SCHEME.test(url.protocol)) return null;
  let pathname = decode(url.pathname);
  const line = lineFromFragment(url.hash.replace(/^#/, ""));
  if (pathname === "" || pathname.endsWith("/")) return null;
  if (url.hostname !== "" && url.hostname.toLowerCase() !== "localhost") {
    const unc = `\\\\${url.hostname}${pathname.replaceAll("/", "\\")}`;
    return { path: normalizePath(unc), line };
  }
  // `file:///C:/repo/a.ts` keeps a leading slash before the drive.
  if (/^\/[A-Za-z]:\//.test(pathname)) pathname = pathname.slice(1);
  return { path: normalizePath(pathname), line };
}

/**
 * Resolve a link from chat output into a workspace file.
 *
 * @param href - The raw `href` attribute.
 * @param baseDir - Directory relative links resolve against.
 * @param workspaceRoot - Workspace path; paths outside it are left alone.
 */
export function resolveFileHref(
  href: string,
  baseDir: string,
  workspaceRoot: string,
): FileLinkTarget | null {
  const raw = href.trim();
  if (raw === "" || raw.startsWith("#")) return null;
  if (FILE_SCHEME.test(raw)) {
    const target = fromFileUrl(raw);
    return target !== null && isInsidePath(target.path, workspaceRoot) ? target : null;
  }
  if (IGNORED_SCHEME.test(raw)) return null;
  // Protocol-relative URLs (`//example.com/a`) are not node paths.
  if (raw.startsWith("//")) return null;
  // Unknown schemes are URLs; drive paths (`C:/repo`) are not schemes.
  if (SCHEME.test(raw) && !DRIVE_PATH.test(raw)) return null;

  const hashIndex = raw.indexOf("#");
  const fragment = hashIndex === -1 ? "" : raw.slice(hashIndex + 1);
  let withoutFragment = hashIndex === -1 ? raw : raw.slice(0, hashIndex);
  const queryIndex = withoutFragment.indexOf("?");
  if (queryIndex !== -1) withoutFragment = withoutFragment.slice(0, queryIndex);

  const suffix = splitLineSuffix(withoutFragment);
  const line = lineFromFragment(fragment) ?? suffix.line;
  const path = decode(suffix.path);
  if (path === "" || path.endsWith("/")) return null;

  const resolved = isAbsolutePath(path)
    ? normalizePath(path)
    : normalizePath(joinPath(baseDir, path));
  if (!isInsidePath(resolved, workspaceRoot)) return null;
  return { path: resolved, line };
}
