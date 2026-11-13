import { describe, expect, it } from "vitest";
import type { SessionSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  isWaitingSessionStatus,
  sessionAttentionBadgeLabel,
  sessionNeedsAttention,
} from "./session-status.ts";

function makeSession(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: "s-1",
    project_id: "github.com/nazo6/flexagent",
    node_id: "n-1",
    local_path: "/repo",
    git_branch: "main",
    is_worktree: false,
    agent_id: "opencode2",
    agent_session_id: null,
    parent_session_id: null,
    title: "Session",
    status: "idle",
    current_mode: null,
    usage: null,
    last_node_seq: 1,
    fork_scope: "none",
    created_at: 100,
    updated_at: 100,
    archived_at: null,
    ...overrides,
  };
}

describe("session-status helpers", () => {
  it("identifies waiting statuses", () => {
    expect(isWaitingSessionStatus("waiting_permission")).toBe(true);
    expect(isWaitingSessionStatus("waiting_input")).toBe(true);
    expect(isWaitingSessionStatus("running")).toBe(false);
    expect(isWaitingSessionStatus("idle")).toBe(false);
    expect(isWaitingSessionStatus("error")).toBe(false);
    expect(isWaitingSessionStatus("stopped")).toBe(false);
    expect(isWaitingSessionStatus("provisioning")).toBe(false);
  });

  it("keeps waiting sessions needing attention even when archived", () => {
    expect(sessionNeedsAttention(makeSession({ status: "waiting_permission" }))).toBe(true);
    expect(
      sessionNeedsAttention(makeSession({ status: "waiting_permission", archived_at: 200 })),
    ).toBe(true);
    expect(sessionNeedsAttention(makeSession({ status: "waiting_input", archived_at: 200 }))).toBe(
      true,
    );
  });

  it("surfaces error sessions only while unarchived", () => {
    expect(sessionNeedsAttention(makeSession({ status: "error", archived_at: null }))).toBe(true);
    expect(sessionNeedsAttention(makeSession({ status: "error", archived_at: 200 }))).toBe(false);
    expect(sessionNeedsAttention(makeSession({ status: "running" }))).toBe(false);
    expect(sessionNeedsAttention(makeSession({ status: "idle" }))).toBe(false);
    expect(sessionNeedsAttention(makeSession({ status: "stopped" }))).toBe(false);
  });

  it("returns compact attention badge labels only for attention statuses", () => {
    expect(sessionAttentionBadgeLabel("waiting_permission")).toBe(m.shell_badge_approval());
    expect(sessionAttentionBadgeLabel("waiting_input")).toBe(m.shell_badge_input());
    expect(sessionAttentionBadgeLabel("error")).toBe(m.status_error());
    expect(sessionAttentionBadgeLabel("running")).toBeNull();
    expect(sessionAttentionBadgeLabel("idle")).toBeNull();
    expect(sessionAttentionBadgeLabel("stopped")).toBeNull();
    expect(sessionAttentionBadgeLabel("provisioning")).toBeNull();
  });
});
