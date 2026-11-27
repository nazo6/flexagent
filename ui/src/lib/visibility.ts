/**
 * Runs `callback` when the document becomes hidden.
 *
 * Returns a cleanup function for `onMount` / `onDestroy`. The callback reads no
 * reactive state itself; callers capture the current draft ID when it runs.
 */
export function onDocumentHidden(callback: () => void): () => void {
  const handler = () => {
    if (document.visibilityState === "hidden") callback();
  };
  document.addEventListener("visibilitychange", handler);
  return () => document.removeEventListener("visibilitychange", handler);
}
