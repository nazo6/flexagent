/** A quick-jump folder (recent project / session). */
export interface QuickPath {
  /** Display label. */
  label: string;
  /** Absolute folder path. */
  path: string;
  /** Optional secondary line. */
  description?: string | null;
}

/** A breadcrumb segment of an absolute path. */
export interface PathBreadcrumb {
  /** Display label (the final path segment, or the root/drive). */
  label: string;
  /** Absolute path to navigate to. */
  path: string;
}

/**
 * Builds clickable breadcrumbs for Unix, Windows drive and UNC paths.
 *
 * Returns an empty list for the Windows drive-list view (empty path).
 */
export function buildBreadcrumbs(path: string): PathBreadcrumb[] {
  if (path === "") return [];
  const windows = path.includes("\\");
  if (!windows) {
    const crumbs: PathBreadcrumb[] = [{ label: "/", path: "/" }];
    let current = "";
    for (const part of path.split("/").filter((segment) => segment !== "")) {
      current += `/${part}`;
      crumbs.push({ label: part, path: current });
    }
    return crumbs;
  }

  const parts = path.split(/[\\/]/).filter((segment) => segment !== "");
  if (parts.length === 0) return [];
  if (path.startsWith("\\\\")) {
    // UNC path: \\server\share\...
    const [server, share, ...rest] = parts;
    if (server === undefined || share === undefined) return [];
    const root = `\\\\${server}\\${share}`;
    const crumbs: PathBreadcrumb[] = [{ label: `\\\\${server}`, path: root }];
    let current = root;
    for (const part of rest) {
      current += `\\${part}`;
      crumbs.push({ label: part, path: current });
    }
    return crumbs;
  }

  // Windows drive path: C:\Users\...
  const [drive, ...rest] = parts;
  if (drive === undefined) return [];
  const crumbs: PathBreadcrumb[] = [{ label: drive, path: `${drive}\\` }];
  let current = `${drive}\\`;
  for (const part of rest) {
    current += part;
    crumbs.push({ label: part, path: current });
    current += "\\";
  }
  return crumbs;
}

/** Whether an entry name matches the folder filter (case-insensitive). */
export function matchesFolderFilter(name: string, filter: string): boolean {
  const needle = filter.trim().toLowerCase();
  return needle === "" || name.toLowerCase().includes(needle);
}
