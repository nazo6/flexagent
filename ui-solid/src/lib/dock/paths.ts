/** Path helpers for node paths (the browser does not know the node's OS). */

/** Separator used by a node path (Windows when it only carries backslashes). */
function separatorOf(path: string): "/" | "\\" {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

/** Basename for both POSIX and Windows paths. */
export function basename(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const index = normalized.lastIndexOf("/");
  return index === -1 ? normalized : normalized.slice(index + 1);
}

/** Directory containing a node path (POSIX or Windows). */
export function dirname(path: string): string {
  const separator = separatorOf(path);
  const normalized = path.replaceAll(separator === "\\" ? "/" : "\\", separator);
  const index = normalized.lastIndexOf(separator);
  if (index === -1) return ".";
  if (index === 0) return normalized.slice(0, 1);
  const head = normalized.slice(0, index);
  // The parent of a file directly under a drive root keeps the root separator.
  if (/^[A-Za-z]:$/.test(head)) return `${head}${separator}`;
  return head;
}

/** Joins an absolute node path with a relative path using its separator. */
export function joinPath(root: string, relative: string): string {
  if (isAbsolutePath(relative)) return relative;
  const separator = separatorOf(root);
  let cleaned = relative.replace(/^[/\\]+/, "");
  cleaned = separator === "\\" ? cleaned.replaceAll("/", "\\") : cleaned.replaceAll("\\", "/");
  if (root.endsWith("/") || root.endsWith("\\")) return `${root}${cleaned}`;
  return `${root}${separator}${cleaned}`;
}

/** Collapses `.` / `..` segments and duplicate separators without touching the node. */
export function normalizePath(path: string): string {
  if (path === "") return "";
  const unc = /^\\\\[^\\/]+[\\/][^\\/]+/.exec(path);
  if (unc !== null) {
    const head = unc[0].replaceAll("/", "\\");
    const rest = normalizeSegments(path.slice(unc[0].length), "\\", true);
    return rest === "" ? head : `${head}\\${rest}`;
  }
  if (/^[A-Za-z]:[\\/]/.test(path) || /^[A-Za-z]:$/.test(path)) {
    const head = `${path.slice(0, 1)}:\\`;
    const rest = normalizeSegments(path.slice(2), "\\", true);
    return `${head}${rest}`;
  }
  const separator = separatorOf(path);
  const absolute = path.startsWith("/");
  const rest = normalizeSegments(path, separator, absolute);
  if (!absolute) return rest === "" ? "." : rest;
  return rest === "" ? separator : `${separator}${rest}`;
}

function normalizeSegments(path: string, separator: string, absolute: boolean): string {
  const segments: string[] = [];
  for (const segment of path.split(/[\\/]+/)) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (segments.length > 0 && segments[segments.length - 1] !== "..") {
        segments.pop();
      } else if (!absolute) {
        segments.push("..");
      }
      continue;
    }
    segments.push(segment);
  }
  return segments.join(separator);
}

/** Whether a path is inside a root (both sides normalized). */
export function isInsidePath(path: string, root: string): boolean {
  const normalizedPath = normalizePath(path).replaceAll("\\", "/");
  const normalizedRoot = normalizePath(root).replaceAll("\\", "/");
  if (normalizedRoot === "") return false;
  const caseInsensitive = /^[A-Za-z]:\//.test(normalizedRoot) || normalizedRoot.startsWith("//");
  const candidate = normalizedPath.replace(/\/+$/, "");
  const baseText = normalizedRoot.replace(/\/+$/, "");
  const base = baseText === "" ? "/" : baseText;
  const comparable = caseInsensitive ? candidate.toLowerCase() : candidate;
  const normalizedBase = caseInsensitive ? base.toLowerCase() : base;
  if (normalizedBase === "/") return comparable.startsWith("/");
  return comparable === normalizedBase || comparable.startsWith(`${normalizedBase}/`);
}

/** Whether a node path is absolute (POSIX or a Windows drive / UNC path). */
export function isAbsolutePath(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}
