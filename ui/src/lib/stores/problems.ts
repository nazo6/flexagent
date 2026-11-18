import { createRoot } from "solid-js";
import type { ErrorInfo } from "#lib/generated/bindings.ts";
import {
  MAX_PROBLEMS,
  type AppProblem,
  type ProblemSeverity,
  buildProblem,
  problemFromInfo,
} from "#lib/stores/problem-model.ts";
import { ReactiveCell } from "#lib/stores/reactive.ts";

/**
 * Central store for user-visible problems.
 *
 * Errors reported here survive navigation and are rendered by `ProblemCenter`,
 * unlike toasts which disappear. Repeated failures with the same context /
 * code / message replace the existing entry instead of piling up.
 */
export class ProblemStore {
  /** Newest first. */
  readonly #problems = new ReactiveCell<AppProblem[]>([]);
  /** Whether the problem center sheet is open. */
  readonly #centerOpen = new ReactiveCell(false);

  #counter = 0;

  get problems(): AppProblem[] {
    return this.#problems.get();
  }

  set problems(value: AppProblem[]) {
    this.#problems.set(value);
  }

  /** Whether the problem center sheet is open. */
  get centerOpen(): boolean {
    return this.#centerOpen.get();
  }

  set centerOpen(value: boolean) {
    this.#centerOpen.set(value);
  }

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

  /** Opens the problem center. */
  openCenter(): void {
    this.centerOpen = true;
  }

  /** Closes the problem center. */
  closeCenter(): void {
    this.centerOpen = false;
  }

  /** Replaces an identical problem (same key) or prepends a new one. */
  #upsert(problem: AppProblem): AppProblem {
    const others = this.problems.filter((candidate) => candidate.key !== problem.key);
    this.problems = [problem, ...others].slice(0, MAX_PROBLEMS);
    return problem;
  }
}

/** Singleton consumed by `ProblemCenter`, stores and the app shell. */
export const problems = createRoot(() => new ProblemStore());

export type { AppProblem, ProblemSeverity };
