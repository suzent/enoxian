import { useEffect, useState } from 'react'
import { Bot } from 'lucide-react'
import { getAgentIcons, agentIconUrl } from '../api'

/**
 * An agent's logo from the ACP Registry, or the generic bot when it has none.
 *
 * Registry icons are single-colour SVGs drawn in `currentColor`. As an `<img>`
 * that colour cannot reach them (they render black whatever the theme), so the
 * icon is a CSS mask over a `currentColor` box: it takes the text colour of
 * wherever it sits, like the lucide icons around it. The SVG is never put into
 * the page as markup.
 *
 * The daemon maps handles to registry entries (`/api/agent-icons`) and serves
 * the icons it cached, so the browser never reaches the registry's CDN.
 */

let index: Promise<Record<string, string>> | null = null
/** Per registry id: whether its icon loaded. A mask whose image fails hides
 *  the box entirely, so an icon is shown only once it is known to load. */
const loaded = new Map<string, Promise<boolean>>()

function iconIndex(): Promise<Record<string, string>> {
  index ??= getAgentIcons().then(r => r.icons ?? {}, () => {
    // An older daemon, or none yet: bots for now, and ask again in a minute
    // rather than on every icon that mounts.
    window.setTimeout(() => { index = null }, 60_000)
    return {}
  })
  return index
}

function preload(id: string): Promise<boolean> {
  let done = loaded.get(id)
  if (!done) {
    done = new Promise<boolean>(resolve => {
      const img = new Image()
      img.onload = () => resolve(true)
      img.onerror = () => { loaded.delete(id); resolve(false) }
      img.src = agentIconUrl(id)
    })
    loaded.set(id, done)
  }
  return done
}

/** Which registry id `name` shows, once known; `id` skips the lookup. */
function useIconId(name: string, id?: string): string | null {
  const [shown, setShown] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    const pick = id ? Promise.resolve(id) : iconIndex().then(icons => icons[name.replace(/^@/, '').toLowerCase()] ?? null)
    pick
      .then(found => (found ? preload(found).then(ok => (ok ? found : null)) : null))
      .then(found => { if (live) setShown(found) })
    return () => { live = false }
  }, [name, id])
  return shown
}

export default function AgentIcon({ name, id, size = 12, className }: {
  /** The agent's chat handle, with or without the @. */
  name: string
  /** A registry entry id, when the caller already knows it. */
  id?: string
  size?: number
  className?: string
}) {
  const icon = useIconId(name, id)
  if (!icon) return <Bot size={size} className={className} aria-hidden="true" />
  const url = `url("${agentIconUrl(icon)}")`
  return (
    <span
      className={`agent-icon ${className ?? ''}`}
      aria-hidden="true"
      data-icon={icon}
      style={{ width: size, height: size, maskImage: url, WebkitMaskImage: url }}
    />
  )
}
