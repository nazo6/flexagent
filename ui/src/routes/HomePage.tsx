import { createEffect } from "solid-js";
import NewSessionChat from "#lib/components/session/NewSessionChat.tsx";
import { m } from "#lib/paraglide/messages.js";

/** Session list / new session entry point (`ui/src/routes/+page.svelte`). */
export default function HomePage() {
  createEffect(
    () => m.shell_new_session_page_title(),
    (title) => {
      document.title = title;
    },
  );

  return (
    <div class="app-scroll-centered h-full overflow-y-auto">
      <NewSessionChat />
    </div>
  );
}
