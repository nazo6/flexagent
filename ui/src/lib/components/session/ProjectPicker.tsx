import { For, Show } from "solid-js";
import PickerPill from "#lib/components/composer/PickerPill.tsx";
import PickerItem from "#lib/components/composer/PickerItem.tsx";
import { m } from "#lib/paraglide/messages.js";
import { ProjectPillIcon } from "./new-session-icons.tsx";

const LINK_ROW =
  "text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]";

export interface ProjectPickerProps {
  /** Value shown in the trigger. */
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  disabled: boolean;
  projects: readonly { project_id: string; name: string }[];
  selectedProjectId: string | null;
  onSelect: (projectId: string) => void;
}

/**
 * Project pill: the registered projects, plus links to register and manage
 * them. The first path candidate (bindings, then recent sessions) depends on
 * the chosen project, so switching also re-resolves the node and the agent.
 */
export default function ProjectPicker(props: ProjectPickerProps) {
  return (
    <PickerPill
      label={m.new_session_project()}
      value={props.label}
      icon={<ProjectPillIcon />}
      open={props.open}
      onOpenChange={(open) => props.onOpenChange(open)}
      disabled={props.disabled}
    >
      <Show
        when={props.projects.length > 0}
        fallback={
          <div class="p-1">
            <a href="/projects" class={LINK_ROW}>
              {m.new_session_register_project()}
            </a>
          </div>
        }
      >
        <div class="max-h-72 overflow-y-auto p-1">
          <For each={props.projects} keyed={(project) => project.project_id}>
            {(project) => (
              <PickerItem
                selected={project().project_id === props.selectedProjectId}
                onSelect={() => props.onSelect(project().project_id)}
              >
                <span class="block truncate">{project().name}</span>
              </PickerItem>
            )}
          </For>
        </div>
        <div class="border-t p-1">
          <a href="/projects" class={LINK_ROW}>
            {m.new_session_manage_projects()}
          </a>
        </div>
      </Show>
    </PickerPill>
  );
}
