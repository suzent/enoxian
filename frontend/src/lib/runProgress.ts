import type { RunProgress } from '../types'

/** What each ACP `ToolKind` is doing, as the activity strip says it. */
const TOOL_VERBS: Record<string, string> = {
  read: 'reading',
  edit: 'editing',
  delete: 'deleting',
  move: 'moving',
  search: 'searching',
  execute: 'running',
  think: 'thinking',
  fetch: 'fetching',
  switch_mode: 'switching mode',
}

/** One short phrase for a running turn's progress, as another device sees it:
 *  the tool call it is in and how far through its plan it is. Undefined when
 *  it has reported nothing worth saying.
 *
 *  No durations: `started_at` and `since` are the running device's clock, and
 *  the strip only re-renders when an activity changes, so an "ago" would be
 *  both skewed and stale. */
export function describeProgress(progress?: RunProgress | null): string | undefined {
  if (!progress) return undefined
  const parts: string[] = []
  const tool = progress.tool
  if (tool) {
    const verb = TOOL_VERBS[tool.kind] ?? 'using a tool'
    const object = tool.kind === 'execute' ? tool.program : tool.path
    const more = progress.open_tools > 1 ? ` +${progress.open_tools - 1}` : ''
    parts.push(`${object ? `${verb} ${object}` : verb}${more}`)
  }
  if (progress.plan_total > 0) {
    const step = `step ${Math.min(progress.plan_done + 1, progress.plan_total)}/${progress.plan_total}`
    parts.push(progress.plan_step ? `${step}: ${progress.plan_step}` : step)
  }
  return parts.length > 0 ? parts.join(' · ') : undefined
}
