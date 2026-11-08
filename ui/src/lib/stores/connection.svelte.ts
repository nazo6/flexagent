import { ApiError } from "#lib/api/errors.ts";
import { ApiClient } from "#lib/api/client.ts";
import type { ConnectionRole, SystemInfoResponse } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** A switchable connection target (per origin). */
export interface ConnectionTarget {
  /** Unique ID (`current` is the current origin). */
  id: string;
  /** Display name (e.g. `Central server (Tailscale)`). */
  label: string;
  /** Origin URL (`''` = current origin, no trailing slash). */
  url: string;
}

const TARGETS_KEY = "fxg.connection.targets";

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
  localStorage.setItem(TARGETS_KEY, JSON.stringify(stored));
}

/**
 * Store that manages the connection target (central server / local node) and auth state.
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

  /** Connection target backend info (null when not fetched). */
  systemInfo = $state<SystemInfoResponse | null>(null);
  /**
   * Connection target type (result of `GET /api/v1/meta`; null when unreachable).
   *
   * Only the auth-free `meta` endpoint is used as the source of truth; do not
   * infer from `system/info` or the hostname (treat an unreachable target as an
   * error rather than guessing).
   */
  role = $state<ConnectionRole | null>(null);
  /** Whether authenticated (Cookie / Bearer established). */
  authenticated = $state(false);
  /** Whether the token input dialog should be shown. */
  tokenRequired = $state(false);
  /** Whether the initial system/info check has completed. */
  ready = $state(false);
  /** Connection error such as network unreachability. */
  error = $state<string | null>(null);
  /** Registered switch targets (excluding the current origin). */
  targets = $state<ConnectionTarget[]>([]);

  readonly client = new ApiClient({ baseUrl: "" });

  constructor() {
    this.targets = loadStoredTargets();
  }

  /**
   * Check in the order `GET /api/v1/meta` (connection type) →
   * `GET /api/v1/system/info` (auth state). Do not guess on a `meta` failure;
   * treat it as a connection error.
   */
  async probe(): Promise<void> {
    try {
      this.role = (await this.client.meta()).role;
      this.systemInfo = await this.client.systemInfo();
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
  }

  /** Validate the token and establish the `fxg_session` Cookie. */
  async login(token: string): Promise<void> {
    const trimmed = token.trim();
    if (!trimmed) throw new Error(m.lib_token_required());
    await this.client.authLogin(trimmed);
    this.tokenRequired = false;
    await this.probe();
    if (!this.authenticated) {
      throw new Error(m.lib_token_accepted_probe_failed());
    }
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

  /** Register a switch target (duplicate URLs are not added). */
  addTarget(label: string, url: string): void {
    const normalized = normalizeUrl(url);
    if (!normalized || normalized === this.origin) return;
    if (this.targets.some((target) => target.url === normalized)) return;
    this.targets = [
      ...this.targets,
      {
        id: `target-${crypto.randomUUID()}`,
        label: label.trim() || normalized,
        url: normalized,
      },
    ];
    persistTargets(this.targets);
  }

  removeTarget(id: string): void {
    this.targets = this.targets.filter((target) => target.id !== id);
    persistTargets(this.targets);
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
