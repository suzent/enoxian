import { describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import ChannelList from '../ChannelList'
import type { Channel } from '../../types'

const channel = (id: string | null, name: string, extra: Partial<Channel> = {}): Channel => ({
  id, name, purpose: '', created_by: 'alice', created_at: 0, archived: false, ...extra,
})

const channels = [
  channel(null, 'general'),
  channel('c-design', 'design', { purpose: 'UI work' }),
  channel('c-old', 'old', { archived: true }),
]

const noop = () => {}
const asyncNoop = async () => {}

describe('ChannelList', () => {
  it('lists live channels, hides archived ones, and marks the current one', () => {
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} />)
    expect(screen.getByRole('button', { name: /general/ })).toHaveAttribute('aria-current', 'true')
    expect(screen.getByRole('button', { name: /^design/ })).not.toHaveAttribute('aria-current')
    expect(screen.queryByText('old')).toBeNull()
  })

  it('keeps an archived channel listed while it is open', () => {
    render(<ChannelList channels={channels} current="c-old" unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} />)
    expect(screen.getByText('old')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Archive #old' })).toBeNull()
  })

  it('shows unread counts for other channels and selects by id', () => {
    const onSelect = vi.fn()
    render(<ChannelList channels={channels} current={null} unread={{ 'c-design': 3 }} onSelect={onSelect} onCreate={asyncNoop} onArchive={asyncNoop} />)
    expect(screen.getByLabelText('3 unread')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /^design/ }))
    expect(onSelect).toHaveBeenCalledWith('c-design')
  })

  it('archives a channel but never #general', () => {
    const onArchive = vi.fn().mockResolvedValue(undefined)
    render(<ChannelList channels={channels} current="c-design" unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={onArchive} />)
    expect(screen.queryByRole('button', { name: 'Archive #general' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Archive #design' }))
    expect(onArchive).toHaveBeenCalledWith('c-design')
  })

  it('creates a channel and reports a rejected name', async () => {
    const onCreate = vi.fn()
      .mockRejectedValueOnce(new Error('channel name taken'))
      .mockResolvedValueOnce(undefined)
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={onCreate} onArchive={asyncNoop} />)
    fireEvent.click(screen.getByRole('button', { name: 'New channel' }))
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'design' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('channel name taken')
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'research' } })
    fireEvent.change(screen.getByLabelText('Channel purpose'), { target: { value: 'papers' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create' }))
    await waitFor(() => expect(screen.queryByLabelText('Channel name')).toBeNull())
    expect(onCreate).toHaveBeenLastCalledWith('research', 'papers')
  })

  it('cancels the form with Escape', () => {
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} />)
    fireEvent.click(screen.getByRole('button', { name: 'New channel' }))
    fireEvent.keyDown(screen.getByLabelText('Channel name'), { key: 'Escape' })
    expect(screen.queryByLabelText('Channel name')).toBeNull()
  })
})
