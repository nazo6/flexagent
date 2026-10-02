import { ApiError } from "$lib/api/errors";
import { ApiClient } from "$lib/api/client";
import type { ConnectionRole } from "$lib/generated/ConnectionRole";
import type { SystemInfoResponse } from "$lib/generated/SystemInfoResponse";

/** 切替可能な接続先 (オリジン単位)。 */
export interface ConnectionTarget {
  /** 一意 ID (`current` は現在のオリジン)。 */
  id: string;
  /** 表示名 (例: `中央サーバー (Tailscale)`)。 */
  label: string;
  /** オリジン URL (`''` = 現在のオリジン。末尾スラッシュなし)。 */
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
 * 接続先 (中央サーバー / ローカルノード) と認証状態を管理するストア。
 *
 * 接続先の切替は**オリジン単位のナビゲーション** (`location.assign`) で行う。
 * クロスオリジンの fetch は共通ミドルウェアの Host/Origin 検証 (CSWSH / DNS
 * Rebinding 対策) と CORS の双方で拒否されるため、同一オリジンで完結させる
 * (`fxg_session` Cookie もオリジンごとに独立)。
 */
export class ConnectionStore {
  /** 現在のオリジン ('' の場合はビルド時)。 */
  readonly origin = typeof location !== "undefined" ? location.origin : "";

  /** 接続先バックエンドの情報 (未取得は null)。 */
  systemInfo = $state<SystemInfoResponse | null>(null);
  /**
   * 接続先種別 (`GET /api/v1/meta` の結果。接続不可時は null)。
   *
   * 認証不要の `meta` のみを情報源とし、`system/info` やホスト名からの推測は
   * 行わない (接続できない場合は推測せずエラーとして扱う)。
   */
  role = $state<ConnectionRole | null>(null);
  /** 認証済み (Cookie / Bearer 確立済み) か。 */
  authenticated = $state(false);
  /** トークン入力ダイアログを表示すべきか。 */
  tokenRequired = $state(false);
  /** 初回の system/info 確認が完了したか。 */
  ready = $state(false);
  /** ネットワーク到達不能などの接続エラー。 */
  error = $state<string | null>(null);
  /** 登録済みの切替先 (現在のオリジンは含まない)。 */
  targets = $state<ConnectionTarget[]>([]);

  readonly client = new ApiClient({ baseUrl: "" });

  constructor() {
    this.targets = loadStoredTargets();
  }

  /**
   * `GET /api/v1/meta` (接続先種別) → `GET /api/v1/system/info` (認証状態)
   * の順で確認する。`meta` の取得失敗は推測せず接続エラーとして扱う。
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

  /** トークンを検証し、`fxg_session` Cookie を確立する。 */
  async login(token: string): Promise<void> {
    const trimmed = token.trim();
    if (!trimmed) throw new Error("トークンを入力してください");
    await this.client.authLogin(trimmed);
    this.tokenRequired = false;
    await this.probe();
    if (!this.authenticated) {
      throw new Error("トークンは受け付けられましたが接続確認に失敗しました");
    }
  }

  /** `fxg_session` Cookie を失効させる。 */
  async logout(): Promise<void> {
    try {
      await this.client.authLogout();
    } finally {
      this.client.setToken(null);
      await this.probe();
    }
  }

  /** 切替先を登録する (同一 URL は重複登録しない)。 */
  addTarget(label: string, url: string): void {
    const normalized = normalizeUrl(url);
    if (!normalized || normalized === this.origin) return;
    if (this.targets.some((target) => target.url === normalized)) return;
    this.targets = [
      ...this.targets,
      { id: `target-${crypto.randomUUID()}`, label: label.trim() || normalized, url: normalized },
    ];
    persistTargets(this.targets);
  }

  removeTarget(id: string): void {
    this.targets = this.targets.filter((target) => target.id !== id);
    persistTargets(this.targets);
  }

  /** 現在のオリジン以外の有効な切替先 (ログアウト状態でも利用可能)。 */
  get availableTargets(): ConnectionTarget[] {
    return this.targets.filter((target) => target.url !== this.origin);
  }

  /** 接続先を切替える (現在のオリジンの UI からそのオリジンの UI へ遷移する)。 */
  switchTo(id: string): void {
    const target = this.targets.find((candidate) => candidate.id === id);
    if (!target || typeof location === "undefined") return;
    if (target.url === this.origin) return;
    location.assign(target.url);
  }
}
