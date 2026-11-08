import type { ErrorInfo } from "#lib/generated/bindings.ts";
import {
  MAX_PROBLEMS,
  type AppProblem,
  type ProblemSeverity,
  buildProblem,
  problemFromInfo,
} from "./problem-model.ts";

/**
 * Central store for user-visible problems.
 *
 * Errors reported here survive navigation and are rendered by `ErrorBanner`,
 * unlike toasts which disappear. Repeated failures with the same context /
 * code / message replace the existing entry instead of piling up.
 */
export class ProblemStore {
  /** Newest first. */
  problems = $state<AppProblem[]>([]);

  #counter = 0;

  /**
   * Reports a failure and returns the stored problem.
   *
   * Accepts `ApiError`, `Error`, strings and arbitrary thrown values.
   */
  report(context: string, error: unknown, severity: ProblemSeverity = "error"): AppProblem {
    const problem = buildProblem(context, error, severity, ++this.#counter);
    return this.#upsert(problem);
  }

  /** Reports a warning message (no associated exception). */
  warn(context: string, message: string): AppProblem {
    return this.report(context, new Error(message), "warning");
  }

  /** Reports a structured error body (`ErrorInfo`) that is not an exception. */
  reportInfo(context: string, info: ErrorInfo, severity: ProblemSeverity = "error"): AppProblem {
    return this.#upsert(problemFromInfo(context, info, severity, ++this.#counter));
  }

  /** Removes one problem by ID. */
  dismiss(id: string): void {
    this.problems = this.problems.filter((problem) => problem.id !== id);
  }

  /** Removes every problem. */
  clear(): void {
    this.problems = [];
  }

  /** Replaces an identical problem (same key) or prepends a new one. */
  #upsert(problem: AppProblem): AppProblem {
    const others = this.problems.filter((candidate) => candidate.key !== problem.key);
    this.problems = [problem, ...others].slice(0, MAX_PROBLEMS);
    return problem;
  }
}

/** Singleton consumed by `ErrorBanner`, stores and `+layout.svelte`. */
export const problems = new ProblemStore();

export type { AppProblem, ProblemSeverity };
