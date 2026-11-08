import type { AgentSource, AgentSummary } from "$lib/generated/bindings";

/** 起動形態の表示ラベル。 */
export function agentSourceLabel(source: AgentSource): string {
  switch (source) {
    case "managed":
      return "fxg 管理";
    case "external":
      return "既存インストール";
    case "package":
      return "npx/uvx";
    case "custom":
      return "カスタム";
  }
}

/** 一覧に出す短い説明 (例: `既存インストール`)。 */
export function agentHint(agent: AgentSummary): string {
  if (agent.custom_config !== null) return "カスタム";
  if (agent.sources.includes("external")) return "既存インストール";
  if (agent.sources.includes("managed")) return "fxg 管理";
  if (agent.sources.includes("package")) return "npx/uvx";
  return "未導入";
}

/** 選択肢用のラベル (表示名 + 起動形態、ID が異なる場合のみ ID を併記)。 */
export function agentOptionLabel(agent: AgentSummary): string {
  const name = agent.name === agent.id ? agent.id : `${agent.name} (${agent.id})`;
  return `${name} — ${agentHint(agent)}`;
}

/** ノードで利用可能なエージェントだけを返す。 */
export function agentsAvailableOn(agents: AgentSummary[], nodeId: string): AgentSummary[] {
  return agents.filter((agent) => agent.installed_nodes.includes(nodeId));
}

/** `npx` / `uvx` 経由での利用が有効になっているか。 */
export function isPackageEnabled(agent: AgentSummary): boolean {
  return agent.sources.includes("package");
}
