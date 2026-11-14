<script lang="ts">
  import type { WorkspaceGitFileStatus } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";

  let { status, class: className = "" }: { status: WorkspaceGitFileStatus; class?: string } =
    $props();

  const label = $derived.by(() => {
    switch (status.kind) {
      case "modified":
        return "M";
      case "added":
        return "A";
      case "deleted":
        return "D";
      case "renamed":
        return "R";
      case "copied":
        return "C";
      case "type_changed":
        return "T";
      case "untracked":
        return "U";
      case "conflicted":
        return "!";
      case "unknown":
        return "?";
    }
  });

  const title = $derived.by(() => {
    switch (status.kind) {
      case "modified":
        return m.dock_git_modified();
      case "added":
        return m.dock_git_added();
      case "deleted":
        return m.dock_git_deleted();
      case "renamed":
        return m.dock_git_renamed();
      case "copied":
        return m.dock_git_copied();
      case "type_changed":
        return m.dock_git_type_changed();
      case "untracked":
        return m.dock_git_untracked();
      case "conflicted":
        return m.dock_git_conflicted();
      case "unknown":
        return m.dock_git_unknown();
    }
  });

  const tone = $derived.by(() => {
    switch (status.kind) {
      case "modified":
        return "bg-warn/15 text-warn";
      case "added":
        return "bg-ok/15 text-ok";
      case "deleted":
      case "conflicted":
        return "bg-danger/15 text-danger";
      case "renamed":
      case "copied":
      case "type_changed":
        return "bg-signal-soft text-signal";
      case "untracked":
      case "unknown":
        return "bg-muted text-muted-foreground";
    }
  });
</script>

<span
  class={cn(
    "inline-flex size-4 shrink-0 items-center justify-center rounded-sm font-mono text-[10px] font-semibold leading-none",
    tone,
    className,
  )}
  {title}
  aria-label={title}
>
  {label}
</span>
