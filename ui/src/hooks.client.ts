import type { HandleClientError } from "@sveltejs/kit/hooks";

/**
 * Logs unexpected client errors.
 *
 * SvelteKit catches unknown errors raised while rendering or running effects,
 * replaces the screen with its fallback error page, and otherwise discards the
 * error without logging it. Without this hook a UI crash leaves no trace in the
 * browser console, which makes it impossible to diagnose. SvelteKit expects this
 * hook to never throw.
 */
export const handleError: HandleClientError = ({ kind, error }) => {
  if (kind === "unknown") {
    console.error("[fxg] unexpected client error", error);
  }
  // Return nothing: keep the status and message of app / framework errors.
};
