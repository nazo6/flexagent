import { Show } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import { m } from "#lib/paraglide/messages.js";
import IconFolderGit2 from "~icons/lucide/folder-git-2";
import IconGitFork from "~icons/lucide/git-fork";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconSparkles from "~icons/lucide/sparkles";
import IconX from "~icons/lucide/x";

/** Title block of the new-session screen. */
export function NewSessionHeader() {
  return (
    <div class="flex flex-col items-center gap-2.5 text-center">
      <div class="bg-signal-soft text-signal flex size-11 items-center justify-center rounded-xl">
        <IconSparkles class="size-5" />
      </div>
      <h1 class="text-2xl font-semibold tracking-tight">{m.new_session_title()}</h1>
      <p class="text-muted-foreground max-w-md text-[13px]">{m.new_session_description()}</p>
    </div>
  );
}

/**
 * Banner for a fork: which session the new one continues, and a way to
 * cancel the inheritance.
 */
export function ForkBanner(props: { sessionId: string; onClear: () => void }) {
  return (
    <div class="bg-signal-soft/70 border-signal/25 flex items-center justify-between gap-2 rounded-lg border p-3 text-[13px]">
      <div class="flex min-w-0 items-center gap-2">
        <IconGitFork class="text-primary size-4 shrink-0" />
        <span class="truncate">
          <code class="bg-background/50 rounded px-1 py-0.5 font-mono">
            {props.sessionId.slice(0, 8)}
          </code>
          — {m.new_session_fork_banner_description()}
        </span>
      </div>
      <Button
        variant="ghost"
        size="sm"
        class="h-6 shrink-0 px-2 text-[13px]"
        onClick={() => props.onClear()}
      >
        <IconX class="mr-1 size-3.5" />
        {m.new_session_fork_clear()}
      </Button>
    </div>
  );
}

/**
 * Notices above the composer: projects still loading, no project registered
 * and a failed project refresh (with a retry).
 */
export function ProjectNotices(props: {
  loading: boolean;
  isEmpty: boolean;
  error: string | null;
  onRetry: () => void;
}) {
  return (
    <>
      <Show when={props.loading && props.isEmpty}>
        <div class="bg-chip text-muted-foreground border-border flex items-center justify-center gap-2 rounded-lg border p-3 text-[13px]">
          <IconLoaderCircle class="size-3.5 animate-spin" />
          {m.common_loading()}
        </div>
      </Show>
      <Show when={!props.loading && props.isEmpty && props.error === null}>
        <div class="border-warn/35 bg-warn-soft/60 flex flex-wrap items-center justify-between gap-2 rounded-lg border p-3 text-[13px]">
          <span class="flex items-center gap-2">
            <IconFolderGit2 class="size-4 shrink-0" />
            {m.new_session_no_projects()}
          </span>
          <a
            href="/projects"
            class="bg-background hover:bg-muted rounded-md border px-2.5 py-1 text-[13px] font-medium"
          >
            {m.new_session_register_project()}
          </a>
        </div>
      </Show>
      <Show when={props.error !== null}>
        <div class="border-danger/30 bg-danger-soft/60 text-danger flex items-center justify-between gap-2 rounded-lg border p-3 text-[13px]">
          <span class="truncate">{props.error}</span>
          <Button variant="ghost" size="xs" onClick={() => props.onRetry()}>
            {m.common_retry()}
          </Button>
        </div>
      </Show>
    </>
  );
}
