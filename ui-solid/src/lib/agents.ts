import type {
  AgentBackendKind,
  AgentSource,
  AgentsResponse,
  AgentSummary,
  RegistryDistributionKind,
} from "#lib/generated/bindings.ts";
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
    case "custom":
      return m.settings_agent_source_custom();
  }
}

/** Display label for a registry distribution form. */
export function registryDistributionLabel(kind: RegistryDistributionKind): string {
  switch (kind) {
    case "binary":
      return "binary";
    case "npx":
      return "npx";
    case "uvx":
      return "uvx";
  }
}

/** Short description shown in lists (for example `Existing install`). */
export function agentHint(agent: AgentSummary): string {
  if (agent.custom_config !== null) return m.settings_agent_source_custom();
  if (agent.sources.includes("external")) return m.settings_agent_source_external();
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

/** Short shared cache for node agent catalogs (identity / icon lookups). */
const CATALOG_TTL_MS = 60_000;
const catalogCache = new Map<string, { at: number; value: Promise<AgentsResponse> }>();

/** Load a node's agent catalog, sharing one request across header mounts. */
export function loadAgentCatalog(
  nodeId: string,
  load: () => Promise<AgentsResponse>,
): Promise<AgentsResponse> {
  const cached = catalogCache.get(nodeId);
  if (cached !== undefined && Date.now() - cached.at < CATALOG_TTL_MS) {
    return cached.value;
  }
  const value = load();
  catalogCache.set(nodeId, { at: Date.now(), value });
  value.catch(() => {
    if (catalogCache.get(nodeId)?.value === value) catalogCache.delete(nodeId);
  });
  return value;
}
