import { describe, expect, it } from "vitest";
import type { AttachmentMeta } from "#lib/generated/bindings.ts";
import { toCreateSessionRequest, type StartSessionForm } from "./start-session-request.ts";

function form(overrides: Partial<StartSessionForm> = {}): StartSessionForm {
  return {
    projectId: "p1",
    provisioner: null,
    nodeId: "n1",
    localPath: "/repo/main",
    worktree: null,
    agentId: "opencode",
    prompt: "hello",
    attachments: [],
    initialConfig: [],
    mode: "default",
    extraArgs: null,
    fork: null,
    commandId: "cmd-1",
    ...overrides,
  };
}

const attachment: AttachmentMeta = {
  file_name: "shot.png",
  mime_type: "image/png",
  size_bytes: 3,
  local_path: null,
  data_b64: null,
};

/**
 * The request branches on where the session runs and whether a new worktree
 * is created; a wrong field here fails after the VM is already provisioned.
 */
describe("toCreateSessionRequest", () => {
  it("runs on the node in the existing directory", () => {
    const request = toCreateSessionRequest(form({ mode: "plan" }));
    expect(request.node_id).toBe("n1");
    expect(request.provisioner).toBeNull();
    expect(request.local_path).toBe("/repo/main");
    expect(request.worktree).toBeNull();
    expect(request.mode).toBe("plan");
    expect(request.initial_prompt).toEqual({ text: "hello", attachments: [] });
  });

  it("drops the local path and the mode for an ephemeral VM", () => {
    const request = toCreateSessionRequest(form({ provisioner: "local-docker", mode: "build" }));
    expect(request.node_id).toBeNull();
    expect(request.provisioner).toBe("local-docker");
    expect(request.local_path).toBeNull();
    expect(request.mode).toBeNull();
  });

  it("sends a new worktree instead of a path, with an empty base branch as null", () => {
    const request = toCreateSessionRequest(
      form({ worktree: { branch: "feat/x", baseBranch: "" } }),
    );
    expect(request.local_path).toBeNull();
    expect(request.worktree).toEqual({ branch: "feat/x", base_branch: null });
  });

  it("omits the initial prompt when there is no text and no attachment", () => {
    expect(toCreateSessionRequest(form({ prompt: "" })).initial_prompt).toBeNull();
    expect(
      toCreateSessionRequest(form({ prompt: "", attachments: [attachment] })).initial_prompt,
    ).toEqual({ text: "", attachments: [attachment] });
  });

  it("keeps the fork target", () => {
    const request = toCreateSessionRequest(form({ fork: { sessionId: "s1", upToNodeSeq: null } }));
    expect(request.fork).toEqual({ from_session_id: "s1", up_to_node_seq: null });
  });
});
