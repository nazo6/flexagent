import { ApiClient } from "#lib/api/client.ts";
import { ApiError } from "#lib/api/errors.ts";
import type { ConnectionRole, SystemInfoResponse } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { ReactiveCell } from "#lib/stores/reactive.ts";
import { randomUuid } from "#lib/uuid.ts";

/** A switchable connection target (per origin). */
export interface ConnectionTarget {
  /** Unique ID (`current` is the current origin). */
  id: string;
  /** Display name (e.g. `Central server (Tailscale)`). */
  label: string;
  /** Origin URL (`''` = current origin, no trailing slash). */
  url: string;
}

const TARGETS_KEY = "fxg:connection_targets";

interface StoredTargets {
  version: 1;
  targets: ConnectionTarget[];
}

function normalizeUrl(url: string): string {
  return url.trim().replace(/\/+$/, "");
}

function loadStoredTargets(): ConnectionTarget[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(TARGETS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as StoredTargets;
    if (parsed.version !== 1 || !Array.isArray(parsed.targets)) return [];
    return parsed.targets.filter(
      (target) =>
        typeof target?.id === "string" &&
        typeof target?.label === "string" &&
        typeof target?.url === "string" &&
        target.id !== "current",
    );
  } catch {
    return [];
  }
}

function persistTargets(targets: ConnectionTarget[]): void {
  if (typeof localStorage === "undefined") return;
  const stored: StoredTargets = { version: 1, targets };
  try {
    localStorage.setItem(TARGETS_KEY, JSON.stringify(stored));
  } catch {
    // Storage may be disabled or full; the target list is a convenience.
  }
}

/**
 * Store that manages the connection target (hub / local node) and auth state.
 *
 * Connection targets are switched by **origin-level navigation**
 * (`location.assign`). Cross-origin fetches are rejected both by the shared
 * middleware's Host/Origin validation (CSWSH / DNS Rebinding protection) and by
 * CORS, so everything stays same-origin (the `fxg_session` Cookie is also
 * independent per origin).
 */
export class ConnectionStore {
  /** Current origin ('' at build time). */
  readonly origin = typeof location !== "undefined" ? location.origin : "";

  readonly client = new ApiClient({ baseUrl: "" });

  readonly #systemInfo = new ReactiveCell<SystemInfoResponse | null>(null);
  readonly #role = new ReactiveCell<ConnectionRole | null>(null);
  readonly #authenticated = new ReactiveCell(false);
  readonly #tokenRequired = new ReactiveCell(false);
  readonly #ready = new ReactiveCell(false);
  readonly #error = new ReactiveCell<string | null>(null);
  readonly #targets = new ReactiveCell<ConnectionTarget[]>(loadStoredTargets());

  /** Connection target backend info (null when not fetched). */
  get systemInfo(): SystemInfoResponse | null {
    return this.#systemInfo.get();
  }

  set systemInfo(value: SystemInfoResponse | null) {
    this.#systemInfo.set(value);
  }

  /**
   * Connection target type (result of `GET /api/v1/meta`; null when unreachable).
   *
   * Only the auth-free `meta` endpoint is used as the source of truth; do not
   * infer from `system/info` or the hostname (treat an unreachable target as an
   * error rather than guessing).
   */
  get role(): ConnectionRole | null {
    return this.#role.get();
  }

  set role(value: ConnectionRole | null) {
    this.#role.set(value);
  }

  /** Whether authenticated (Cookie / Bearer established). */
  get authenticated(): boolean {
    return this.#authenticated.get();
  }

  set authenticated(value: boolean) {
    this.#authenticated.set(value);
  }

  /** Whether the token input dialog should be shown. */
  get tokenRequired(): boolean {
    return this.#tokenRequired.get();
  }

  set tokenRequired(value: boolean) {
    this.#tokenRequired.set(value);
  }

  /** Whether the initial system/info check has completed. */
  get ready(): boolean {
    return this.#ready.get();
  }

  set ready(value: boolean) {
    this.#ready.set(value);
  }

  /** Connection error such as network unreachability. */
  get error(): string | null {
    return this.#error.get();
  }

  set error(value: string | null) {
    this.#error.set(value);
  }

  /** Registered switch targets (excluding the current origin). */
  get targets(): ConnectionTarget[] {
    return this.#targets.get();
  }

  set targets(value: ConnectionTarget[]) {
    this.#targets.set(value);
  }

  /**
   * Check in the order `GET /api/v1/meta` (connection type) →
   * `GET /api/v1/system/info` (auth state). Do not guess on a `meta` failure;
   * treat it as a connection error.
   *
   * Returns whether the target is authenticated, so callers do not have to
   * re-read the state they just caused (Solid state is committed on the next
   * flush).
   */
  async probe(): Promise<boolean> {
    let authenticated = false;
    try {
      this.role = (await this.client.meta()).role;
      this.systemInfo = await this.client.systemInfo();
      authenticated = true;
      this.authenticated = true;
      this.tokenRequired = false;
      this.error = null;
    } catch (err) {
      this.systemInfo = null;
      this.authenticated = false;
      if (err instanceof ApiError && err.isUnauthorized) {
        this.tokenRequired = true;
        this.error = null;
      } else {
        this.tokenRequired = false;
        this.error = err instanceof Error ? err.message : String(err);
      }
    } finally {
      this.ready = true;
    }
    return authenticated;
  }

  /** Validate the token and establish the `fxg_session` Cookie. */
  async login(token: string): Promise<void> {
    const trimmed = token.trim();
    if (!trimmed) throw new Error(m.lib_token_required());
    await this.client.authLogin(trimmed);
    this.tokenRequired = false;
    const authenticated = await this.probe();
    if (!authenticated) {
      throw new Error(m.lib_token_accepted_probe_failed());
    }
  }

  /**
   * Apply a token that the server already accepted (e.g. the auth token rotate
   * response, which also sets the `fxg_session` cookie atomically).
   */
  async applyToken(token: string): Promise<void> {
    this.client.setToken(token);
    this.tokenRequired = false;
    await this.probe();
  }

  /** Invalidate the `fxg_session` Cookie. */
  async logout(): Promise<void> {
    try {
      await this.client.authLogout();
    } finally {
      this.client.setToken(null);
      await this.probe();
    }
  }

  /** Register a switch target. Returns false when it was a duplicate / the current origin. */
  addTarget(label: string, url: string): boolean {
    const normalized = normalizeUrl(url);
    if (!normalized || normalized === this.origin) return false;
    const current = this.targets;
    if (current.some((target) => target.url === normalized)) return false;
    const next: ConnectionTarget[] = [
      ...current,
      {
        id: `target-${randomUuid()}`,
        label: label.trim() || normalized,
        url: normalized,
      },
    ];
    this.targets = next;
    persistTargets(next);
    return true;
  }

  removeTarget(id: string): void {
    const next = this.targets.filter((target) => target.id !== id);
    this.targets = next;
    persistTargets(next);
  }

  /** Valid switch targets other than the current origin (usable even when logged out). */
  get availableTargets(): ConnectionTarget[] {
    return this.targets.filter((target) => target.url !== this.origin);
  }

  /** Switch the connection target (navigate from the current origin's UI to that origin's UI). */
  switchTo(id: string): void {
    const target = this.targets.find((candidate) => candidate.id === id);
    if (!target || typeof location === "undefined") return;
    if (target.url === this.origin) return;
    location.assign(target.url);
  }
}
