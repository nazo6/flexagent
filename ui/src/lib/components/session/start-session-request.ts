import type {
  AttachmentMeta,
  CreateSessionRequest,
  InitialConfig,
} from "#lib/generated/bindings.ts";
import type { SessionModePreference } from "#lib/session-prefs.ts";

/** Everything the start button gathered from the form. */
export interface StartSessionForm {
  projectId: string;
  /** Provisioner name when the session runs in an ephemeral VM. */
  provisioner: string | null;
  nodeId: string;
  /** Working directory for resident-node sessions (no new worktree). */
  localPath: string;
  /** New worktree to create instead of using `localPath`. */
  worktree: { branch: string; baseBranch: string } | null;
  agentId: string;
  prompt: string;
  attachments: readonly AttachmentMeta[];
  initialConfig: InitialConfig[];
  mode: SessionModePreference;
  extraArgs: string[] | null;
  fork: { sessionId: string; upToNodeSeq: number | null } | null;
  commandId: string;
}

/**
 * Build the create-session request from the form values.
 *
 * Pure so the request shape can be checked without a renderer: the branching
 * (provisioner vs node, new worktree vs existing directory, fork) is where a
 * mistyped field turns into a failed session.
 */
export function toCreateSessionRequest(form: StartSessionForm): CreateSessionRequest {
  const useProvisioner = form.provisioner !== null;
  return {
    command_id: form.commandId,
    project_id: form.projectId,
    node_id: useProvisioner ? null : form.nodeId,
    provisioner: form.provisioner,
    local_path: useProvisioner || form.worktree !== null ? null : form.localPath,
    worktree:
      form.worktree === null
        ? null
        : {
            branch: form.worktree.branch,
            base_branch: form.worktree.baseBranch === "" ? null : form.worktree.baseBranch,
          },
    agent_id: form.agentId,
    initial_prompt:
      form.prompt === "" && form.attachments.length === 0
        ? null
        : { text: form.prompt, attachments: [...form.attachments] },
    initial_config: form.initialConfig,
    mode: useProvisioner || form.mode === "default" ? null : form.mode,
    extra_args: form.extraArgs,
    fork:
      form.fork === null
        ? null
        : {
            from_session_id: form.fork.sessionId,
            up_to_node_seq: form.fork.upToNodeSeq,
          },
  };
}
