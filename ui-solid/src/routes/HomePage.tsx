import { m } from "#lib/paraglide/messages.js";

/**
 * Session list / new session entry point.
 *
 * Placeholder until the session port lands; keeps the route table complete so
 * navigation and deep links can be verified end to end.
 */
export default function HomePage() {
  return (
    <div class="desk-page">
      <h1 class="text-2xl font-semibold tracking-tight">{m.shell_new_session_page_title()}</h1>
      <p class="text-muted-ink text-sm">Session port in progress.</p>
    </div>
  );
}
