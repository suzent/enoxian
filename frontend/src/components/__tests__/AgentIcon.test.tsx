import { render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const getAgentIcons = vi.fn(async () => ({ icons: { claude: 'claude-acp', broken: 'gone' } }))
vi.mock('../../api', () => ({
  getAgentIcons,
  agentIconUrl: (id: string) => `/api/agent-icons/${id}.svg`,
}))

// jsdom never loads images; this one loads every icon but `gone`.
class FakeImage {
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  set src(url: string) {
    setTimeout(() => (url.includes('/gone.') ? this.onerror?.() : this.onload?.()))
  }
}

describe('AgentIcon', () => {
  beforeEach(() => { vi.stubGlobal('Image', FakeImage) })
  afterEach(() => { vi.unstubAllGlobals() })

  it('shows a handle’s registry logo as a mask, in the text colour', async () => {
    const { default: AgentIcon } = await import('../AgentIcon')
    const { container } = render(<AgentIcon name="@Claude" />)
    await waitFor(() => expect(container.querySelector('.agent-icon')).not.toBeNull())
    const icon = container.querySelector<HTMLElement>('.agent-icon')!
    expect(icon.dataset.icon).toBe('claude-acp')
    expect(icon.getAttribute('style')).toContain('/api/agent-icons/claude-acp.svg')
  })

  it('keeps the generic bot for unknown handles and icons that fail to load', async () => {
    const { default: AgentIcon } = await import('../AgentIcon')
    const { container } = render(<><AgentIcon name="my-own-agent" /><AgentIcon name="broken" /></>)
    await new Promise(resolve => setTimeout(resolve, 20))
    expect(container.querySelector('.agent-icon')).toBeNull()
    expect(container.querySelectorAll('svg.lucide-bot')).toHaveLength(2)
  })
})
