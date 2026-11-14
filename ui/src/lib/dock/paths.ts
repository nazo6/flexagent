/** Path helpers for node paths (the browser does not know the node's OS). */

/** Basename for both POSIX and Windows paths. */
export function basename(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const index = normalized.lastIndexOf("/");
  return index === -1 ? normalized : normalized.slice(index + 1);
}

/** Joins an absolute node path with a relative path using its separator. */
export function joinPath(root: string, relative: string): string {
  if (isAbsolutePath(relative)) return relative;
  const separator = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  let cleaned = relative.replace(/^[/\\]+/, "");
  cleaned = separator === "\\" ? cleaned.replaceAll("/", "\\") : cleaned.replaceAll("\\", "/");
  if (root.endsWith("/") || root.endsWith("\\")) return `${root}${cleaned}`;
  return `${root}${separator}${cleaned}`;
}

/** Whether a node path is absolute (POSIX or a Windows drive / UNC path). */
export function isAbsolutePath(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}
