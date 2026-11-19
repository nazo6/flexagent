import { createEffect, type Accessor } from "solid-js";

/**
 * Keeps `document.title` in sync with a route's title.
 *
 * `restore` puts the previous title back when the route unmounts: screens that
 * render under a shell (settings, error) would otherwise leave their own title
 * behind after navigating away.
 */
export function useDocumentTitle(
  title: Accessor<string>,
  options: { suffix?: string; restore?: boolean } = {},
): void {
  createEffect(
    () => title(),
    (current) => {
      if (typeof document === "undefined") return;
      const previous = document.title;
      document.title = options.suffix === undefined ? current : `${current}${options.suffix}`;
      if (!options.restore) return;
      return () => {
        document.title = previous;
      };
    },
  );
}
