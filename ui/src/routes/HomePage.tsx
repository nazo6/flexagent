import NewSessionChat from "#lib/components/session/NewSessionChat.tsx";
import { useDocumentTitle } from "#lib/document-title.ts";
import { m } from "#lib/paraglide/messages.js";

/** Session list / new session entry point (`ui/src/routes/+page.svelte`). */
export default function HomePage() {
  useDocumentTitle(() => m.shell_new_session_page_title());

  return (
    <div class="app-scroll-centered h-full overflow-y-auto">
      <NewSessionChat />
    </div>
  );
}
