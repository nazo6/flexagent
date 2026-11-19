import { createMemo, createSignal } from "solid-js";
import type { ConfigOptionValue, InitialConfig, ModeInfo } from "#lib/generated/bindings.ts";
import { loadNewSessionPrefs, type SessionModePreference } from "#lib/session-prefs.ts";

/**
 * Initial mode of a new session.
 *
 * Mode IDs are agent-specific, so the choices come from the probe: a mode the
 * agent does not report is replaced by its default instead of failing
 * `session/set_mode`. Until the probe answers (`null`), the user's (or
 * remembered) choice is passed through unchanged.
 */
export function sessionModeFor(
  proposed: SessionModePreference,
  modes: readonly ModeInfo[] | null,
): SessionModePreference {
  if (proposed === "default" || modes === null) return proposed;
  return modes.some((mode) => mode.mode_id === proposed) ? proposed : "default";
}

/** URL of a session created from the new-session screen. */
export function sessionPath(sessionId: string): string {
  return `/sessions/${sessionId}`;
}

/**
 * The requested mode plus the signal that remembers a user choice.
 *
 * The choice is not persisted here: the screen saves it together with the
 * agent (`persistPrefs`) so the two stay consistent.
 */
export function createSessionModeChoice(options: {
  /** Selected project (the mode is remembered per project). */
  projectId: () => string;
  /** Modes the probed agent reported (`null` until the probe answers). */
  availableModes: () => readonly ModeInfo[] | null;
}) {
  const [chosen, setChosen] = createSignal<SessionModePreference | null>(null);

  const remembered = createMemo((): SessionModePreference => {
    const projectId = options.projectId();
    return projectId === "" ? "default" : (loadNewSessionPrefs(projectId)?.mode ?? "default");
  });

  const current = createMemo(() =>
    sessionModeFor(chosen() ?? remembered(), options.availableModes()),
  );

  return { chosen, setChosen, current };
}

/**
 * Initial config sent with the create request.
 *
 * Only options of a completed probe for the current target are sent: the agent
 * rejects the whole `set_config_option` call when a model is unknown, and a
 * missing value (the agent keeps its default) fails more gracefully.
 */
export function probeInitialConfig(options: {
  probeReady: boolean;
  values: Record<string, ConfigOptionValue>;
}): InitialConfig[] {
  if (!options.probeReady) return [];
  return Object.entries(options.values).map(([key, value]) => ({ key, value }));
}
