import { For, Show } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import PickerPill from "#lib/components/composer/PickerPill.tsx";
import PickerItem from "#lib/components/composer/PickerItem.tsx";
import type { NodeSummary } from "#lib/generated/bindings.ts";
import type { PathCandidate } from "#lib/new-session.ts";
import { m } from "#lib/paraglide/messages.js";
import { DirectoryPillIcon } from "./new-session-icons.tsx";
import { candidateDetail, candidateLabel, pathBasename } from "./new-session-candidates.ts";

const FOOTER_BUTTON =
  "text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]";

export interface DirectoryPickerProps {
  /** Value shown in the trigger. */
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Ephemeral VMs clone into a fresh VM, so there is nothing to pick. */
  provisionerMode: boolean;
  disabled: boolean;
  pathMode: "existing" | "new_worktree";
  onPathModeChange: (mode: "existing" | "new_worktree") => void;
  branch: string;
  onBranchChange: (branch: string) => void;
  baseBranch: string;
  onBaseBranchChange: (branch: string) => void;
  /** A typed / browsed path replaces the candidate list. */
  customPath: boolean;
  typedPath: string;
  onTypedPathChange: (path: string) => void;
  localPath: string;
  selectedCandidate: PathCandidate | null;
  candidates: readonly PathCandidate[];
  onSelectCandidate: (path: string) => void;
  onResetToDefault: () => void;
  onBrowse: () => void;
  /** Switch to the free-form input, starting from the current selection. */
  onEnterPath: () => void;
  projectName: string;
  /** Nodes other than the current one with a binding for the project. */
  otherNodes: readonly NodeSummary[];
  onSelectNode: (nodeId: string) => void;
}

/**
 * Working-directory pill: existing candidates, a free-form path, or a new
 * worktree created from a base branch.
 */
export default function DirectoryPicker(props: DirectoryPickerProps) {
  const existing = () => props.pathMode === "existing";
  return (
    <PickerPill
      label={m.new_session_working_directory()}
      value={props.label}
      icon={<DirectoryPillIcon />}
      contentClass="w-[min(24rem,calc(100vw-1.5rem))]"
      open={props.open}
      onOpenChange={(open) => props.onOpenChange(open)}
      disabled={props.disabled}
    >
      <Show when={props.provisionerMode}>
        <p class="text-muted-foreground p-2 text-[13px]">{m.new_session_ephemeral_note()}</p>
      </Show>

      <Show when={!props.provisionerMode && props.pathMode === "new_worktree"}>
        <div class="flex flex-col gap-2 p-2">
          <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-[11px]">{m.new_session_new_worktree()}</span>
            <Input
              value={props.branch}
              onValueChange={(value) => props.onBranchChange(value)}
              placeholder="feat/new-task"
              class="h-8 text-[13px]"
            />
          </div>
          <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-[11px]">{m.new_session_base_branch()}</span>
            <Input
              value={props.baseBranch}
              onValueChange={(value) => props.onBaseBranchChange(value)}
              placeholder="main"
              class="h-8 text-[13px]"
            />
          </div>
          <p class="text-muted-foreground text-[11px]">{m.new_session_worktree_description()}</p>
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground self-start text-[11px] underline"
            onClick={() => {
              props.onPathModeChange("existing");
              props.onOpenChange(false);
            }}
          >
            {m.new_session_back_to_list()}
          </button>
        </div>
      </Show>

      <Show when={!props.provisionerMode && existing() && props.customPath}>
        <div class="flex flex-col gap-2 p-2">
          <Input
            value={props.typedPath}
            onValueChange={(value) => props.onTypedPathChange(value)}
            placeholder={m.new_session_path_placeholder()}
            class="h-8 font-mono text-[13px]"
            autofocus
          />
          <div class="flex flex-wrap items-center gap-1.5">
            <Button variant="outline" size="sm" onClick={() => props.onBrowse()}>
              {m.new_session_browse()}
            </Button>
            <Show when={props.candidates.length > 0}>
              <Button variant="ghost" size="sm" onClick={() => props.onResetToDefault()}>
                {m.new_session_back_to_list()}
              </Button>
            </Show>
          </div>
          <Show when={props.localPath.trim() !== "" && props.selectedCandidate === null}>
            <p class="text-muted-foreground text-[11px]">{m.new_session_unregistered_path()}</p>
          </Show>
          <Show when={props.selectedCandidate !== null && !props.selectedCandidate.exists}>
            <p class="text-destructive text-[11px]">{m.new_session_path_not_exists()}</p>
          </Show>
        </div>
      </Show>

      <Show
        when={
          !props.provisionerMode && existing() && !props.customPath && props.candidates.length > 0
        }
      >
        <div class="max-h-72 overflow-y-auto p-1">
          <For each={props.candidates} keyed={(candidate) => candidate.path}>
            {(candidate) => (
              <PickerItem
                selected={candidate().path === props.localPath}
                onSelect={() => props.onSelectCandidate(candidate().path)}
              >
                <span class="block truncate">{pathBasename(candidate().path)}</span>
                <span
                  class={`text-muted-foreground block truncate text-[11px] ${candidate().exists ? "" : "text-destructive"}`}
                  title={candidateLabel(candidate())}
                >
                  {candidate().path} · {candidateDetail(candidate())}
                </span>
              </PickerItem>
            )}
          </For>
        </div>
        <div class="flex flex-wrap items-center gap-1 border-t p-1">
          <button type="button" class={FOOTER_BUTTON} onClick={() => props.onBrowse()}>
            {m.new_session_browse()}
          </button>
          <button type="button" class={FOOTER_BUTTON} onClick={() => props.onEnterPath()}>
            {m.new_session_enter_path()}
          </button>
          <button
            type="button"
            class={FOOTER_BUTTON}
            onClick={() => {
              props.onPathModeChange("new_worktree");
              props.onOpenChange(false);
            }}
          >
            {m.new_session_new_worktree()}
          </button>
        </div>
      </Show>

      <Show
        when={
          !props.provisionerMode && existing() && !props.customPath && props.candidates.length === 0
        }
      >
        <div class="flex flex-col gap-2 p-2">
          <p class="text-muted-foreground text-[13px]">
            {m.new_session_no_candidate({
              project: props.projectName === "" ? m.new_session_this_project() : props.projectName,
            })}
          </p>
          <div class="flex flex-wrap items-center gap-1.5">
            <Button variant="outline" size="sm" onClick={() => props.onBrowse()}>
              {m.new_session_select_folder()}
            </Button>
            <a
              href="/projects"
              class="text-muted-foreground hover:text-foreground text-[11px] underline underline-offset-2"
            >
              {m.new_session_go_to_project_settings()}
            </a>
          </div>
          <For each={props.otherNodes} keyed={(node) => node.node_id}>
            {(node) => (
              <button
                type="button"
                class="text-muted-foreground hover:text-foreground self-start text-[11px] underline"
                onClick={() => props.onSelectNode(node().node_id)}
              >
                {m.new_session_registered_on_node({ node: node().name })}
              </button>
            )}
          </For>
        </div>
      </Show>
    </PickerPill>
  );
}
