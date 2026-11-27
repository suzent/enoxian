/**
 * Open this device's settings at one agent's card, from anywhere — a badge in
 * Members, a row in Agent activity — without threading a callback through
 * every panel in between. `App` listens and owns the one settings window.
 */
export const OPEN_AGENT_SETTINGS = 'enoxian:open-agent-settings'

export function openAgentSettings(agent: string): void {
  window.dispatchEvent(new CustomEvent<string>(OPEN_AGENT_SETTINGS, { detail: agent }))
}
