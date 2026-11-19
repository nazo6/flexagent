import { Show } from "solid-js";
import AgentIcon from "#lib/components/agent/AgentIcon.tsx";
import IconBox from "~icons/lucide/box";
import IconFolderGit2 from "~icons/lucide/folder-git-2";
import IconGitBranch from "~icons/lucide/git-branch";
import IconServer from "~icons/lucide/server";
import IconSparkles from "~icons/lucide/sparkles";

/** Leading icon of the project pill. */
export function ProjectPillIcon() {
  return <IconFolderGit2 class="text-muted-foreground size-3.5" />;
}

/** Leading icon of the working-directory pill. */
export function DirectoryPillIcon() {
  return <IconGitBranch class="text-muted-foreground size-3.5" />;
}

/**
 * Leading icon of the environment pill: a box for ephemeral VMs, otherwise a
 * server with the node's availability dot behind it.
 */
export function EnvironmentPillIcon(props: { provisioner: boolean; dotColorClass: string }) {
  return (
    <Show
      when={props.provisioner}
      fallback={
        <span class="relative inline-flex">
          <IconServer class="text-muted-foreground size-3.5" />
          <span
            class={`ring-background absolute -right-1 -bottom-0.5 size-1.5 rounded-full ring-1 ${props.dotColorClass}`}
          />
        </span>
      }
    >
      <IconBox class="text-muted-foreground size-3.5" />
    </Show>
  );
}

/** Leading icon of the agent pill (the agent's own icon when it has one). */
export function AgentPillIcon(props: { icon: string | null }) {
  return <AgentIcon icon={props.icon} class="size-3.5" />;
}

/** Leading icon of the model / config-option pill. */
export function ModelPillIcon() {
  return <IconSparkles class="text-muted-foreground size-3.5" />;
}
