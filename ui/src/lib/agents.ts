import type { AgentBackendKind, AgentSource, AgentSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** Display label for the driver backing an agent. */
export function agentBackendLabel(backend: AgentBackendKind): string {
  switch (backend) {
    case "acp":
      return m.settings_agent_backend_acp();
    case "acp_extended":
      return m.settings_agent_backend_acp_extended();
    case "custom":
      return m.settings_agent_backend_custom();
  }
}

/** Display label for how the agent is launched. */
export function agentSourceLabel(source: AgentSource): string {
  switch (source) {
    case "external":
      return m.settings_agent_source_external();
    case "package":
      return "npx/uvx";
    case "custom":
      return m.settings_agent_source_custom();
  }
}

/** Short description shown in lists (for example `Existing install`). */
export function agentHint(agent: AgentSummary): string {
  if (agent.custom_config !== null) return m.settings_agent_source_custom();
  if (agent.sources.includes("external")) return m.settings_agent_source_external();
  if (agent.sources.includes("package")) return "npx/uvx";
  return m.settings_not_installed();
}

/** Label for select options (display name + launch type; ID included only when it differs). */
export function agentOptionLabel(agent: AgentSummary): string {
  const name = agent.name === agent.id ? agent.id : `${agent.name} (${agent.id})`;
  return `${name} — ${agentHint(agent)}`;
}

/** Return only the agents available on the node. */
export function agentsAvailableOn(agents: AgentSummary[], nodeId: string): AgentSummary[] {
  return agents.filter((agent) => agent.installed_nodes.includes(nodeId));
}

/** Whether usage via `npx` / `uvx` is enabled. */
export function isPackageEnabled(agent: AgentSummary): boolean {
  return agent.sources.includes("package");
}
