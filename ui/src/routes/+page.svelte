<script lang="ts">
  import NewSessionChat from "#lib/components/session/NewSessionChat.svelte";
  import { m } from "#lib/paraglide/messages.js";
</script>

<svelte:head>
  <title>{m.shell_new_session_page_title()}</title>
</svelte:head>

<div class="app-scroll-centered h-full overflow-y-auto">
  <NewSessionChat />
</div>
