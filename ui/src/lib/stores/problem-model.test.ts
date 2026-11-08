import { describe, expect, it } from "vitest";
import { ApiError } from "#lib/api/errors.ts";
import { m } from "#lib/paraglide/messages.js";
import { UNKNOWN_ERROR_MESSAGE, buildProblem } from "./problem-model.ts";

describe("buildProblem", () => {
  it("localizes a structured ApiError and keeps correlation fields", () => {
    const error = new ApiError(
      503,
      {
        code: "NODE_OFFLINE",
        message: "node n1 is offline",
        details: { node_id: "n1" },
        retryable: true,
        recovery: [{ kind: "reconnect_node", node_id: "n1" }],
        request_id: "req-1",
      },
      "node n1 is offline",
    );
    const problem = buildProblem("sessions", error, "warning", 1);
    expect(problem.message).toBe(m.error_node_offline());
    expect(problem.code).toBe("NODE_OFFLINE");
    expect(problem.severity).toBe("warning");
    expect(problem.requestId).toBe("req-1");
    expect(problem.recovery).toEqual([{ kind: "reconnect_node", node_id: "n1" }]);
    expect(problem.details).toContain('"node_id": "n1"');
  });

  it("falls back to the server message for an unknown code", () => {
    const error = ApiError.fromCode(500, "INTERNAL", "boom");
    const problem = buildProblem("save", error, "error", 1);
    expect(problem.message).toBe(m.error_internal());
  });

  it("keeps the stack of a plain Error", () => {
    const problem = buildProblem("save", new Error("disk on fire"), "error", 1);
    expect(problem.message).toBe("disk on fire");
    expect(problem.code).toBeNull();
    expect(problem.details).toContain("disk on fire");
  });

  it("handles non-Error values", () => {
    const problem = buildProblem("save", { reason: "nope" }, "error", 1);
    expect(problem.message).toBe(UNKNOWN_ERROR_MESSAGE);
    expect(problem.details).toContain("nope");
  });

  it("uses the same dedupe key for the same context / code / message", () => {
    const first = buildProblem(
      "sessions",
      ApiError.fromCode(503, "NODE_OFFLINE", "x"),
      "warning",
      1,
    );
    const second = buildProblem(
      "sessions",
      ApiError.fromCode(503, "NODE_OFFLINE", "x"),
      "warning",
      2,
    );
    expect(second.key).toBe(first.key);
  });

  it("includes the context in the dedupe key", () => {
    const first = buildProblem(
      "sessions",
      ApiError.fromCode(503, "NODE_OFFLINE", "x"),
      "warning",
      1,
    );
    const second = buildProblem("nodes", ApiError.fromCode(503, "NODE_OFFLINE", "x"), "warning", 2);
    expect(second.key).not.toBe(first.key);
  });
});
