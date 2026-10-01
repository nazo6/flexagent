/**
 * 新規セッション作成画面の「実行ディレクトリ」候補解決。
 *
 * 既定は **直近使用した場所を優先** する:
 * 1. プロジェクト × ノードの紐付け (`project_node_bindings`) を最終使用順
 * 2. 紐付けに無いパスは直近セッションの `local_path` で補完
 *    (セッション開始時の紐付け登録前に実行されたセッションへのフォールバック)
 */
import type { NodeSummary } from "$lib/generated/NodeSummary";
import type { ProjectSummary } from "$lib/generated/ProjectSummary";
import type { SessionSummary } from "$lib/generated/SessionSummary";

/** 実行ディレクトリの候補 (UI 表示・選択用)。 */
export interface PathCandidate {
  /** 実行ディレクトリの絶対パス */
  path: string;
  /** 候補の出典 (`binding` = プロジェクト紐付け / `session` = セッション履歴) */
  source: "binding" | "session";
  /** Git Worktree か */
  isWorktree: boolean;
  /** 最終確認時の Git ブランチ (セッション履歴のみの候補では起動時スナップショット) */
  gitBranch: string | null;
  /** ソートキー (紐付けは最終使用日時 / セッションは最終更新日時。Unix epoch ms) */
  usedAt: number;
}

/** セッション履歴から補完する候補の上限 (古い場所で候補が溢れないように)。 */
const MAX_SESSION_CANDIDATES = 5;

/**
 * プロジェクト × ノードの実行ディレクトリ候補を直近使用順に組み立てる。
 *
 * 同一パスが紐付けとセッション履歴の両方にある場合は紐付けを優先する
 * (Worktree 判定・ブランチが正データのため)。
 */
export function buildPathCandidates(
  project: ProjectSummary | null,
  nodeId: string,
  sessions: readonly SessionSummary[],
): PathCandidate[] {
  if (!project || nodeId === "") return [];

  const byPath = new Map<string, PathCandidate>();
  for (const binding of project.bindings) {
    if (binding.node_id !== nodeId) continue;
    byPath.set(binding.local_path, {
      path: binding.local_path,
      source: "binding",
      isWorktree: binding.is_worktree,
      gitBranch: binding.git_branch,
      usedAt: binding.last_used_at,
    });
  }

  const recentSessions = sessions
    .filter((session) => session.project_id === project.project_id && session.node_id === nodeId)
    .toSorted((a, b) => b.updated_at - a.updated_at);
  let added = 0;
  for (const session of recentSessions) {
    if (added >= MAX_SESSION_CANDIDATES) break;
    if (byPath.has(session.local_path)) continue;
    byPath.set(session.local_path, {
      path: session.local_path,
      source: "session",
      isWorktree: session.is_worktree,
      gitBranch: session.git_branch,
      usedAt: session.updated_at,
    });
    added += 1;
  }

  return [...byPath.values()].toSorted(
    (a, b) =>
      b.usedAt - a.usedAt ||
      (a.source === b.source ? 0 : a.source === "binding" ? -1 : 1) ||
      Number(a.isWorktree) - Number(b.isWorktree),
  );
}

/**
 * プロジェクト × ノードの最終使用時刻を返す (紐付け・セッション履歴の最大値)。
 *
 * 履歴が無い場合は `0`。
 */
export function projectRecency(
  project: ProjectSummary,
  nodeId: string,
  sessions: readonly SessionSummary[],
): number {
  let latest = 0;
  for (const binding of project.bindings) {
    if (binding.node_id === nodeId) latest = Math.max(latest, binding.last_used_at);
  }
  for (const session of sessions) {
    if (session.project_id === project.project_id && session.node_id === nodeId) {
      latest = Math.max(latest, session.updated_at);
    }
  }
  return latest;
}

/**
 * 既定ノードを選ぶ。
 *
 * オンラインノードのうち、そのプロジェクトの実行履歴 (紐付け・セッション) が
 * 最も新しいノードを選ぶ。履歴が無ければオンライン先頭ノードにフォールバックする
 * (オンラインが無い場合は全ノードを対象にする)。
 */
export function pickDefaultNode(
  nodes: readonly NodeSummary[],
  project: ProjectSummary | null,
  sessions: readonly SessionSummary[],
): string {
  if (nodes.length === 0) return "";
  const online = nodes.filter((node) => node.is_online);
  const candidates = online.length > 0 ? online : nodes;
  if (project) {
    let best: NodeSummary | null = null;
    let bestRecency = 0;
    for (const node of candidates) {
      const recency = projectRecency(project, node.node_id, sessions);
      if (recency > bestRecency) {
        bestRecency = recency;
        best = node;
      }
    }
    if (best !== null) return best.node_id;
  }
  return candidates[0].node_id;
}
