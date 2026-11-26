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
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} onRename={asyncNoop} />)
    expect(screen.getByRole('button', { name: /general/ })).toHaveAttribute('aria-current', 'true')
    expect(screen.getByRole('button', { name: /^design/ })).not.toHaveAttribute('aria-current')
    expect(screen.queryByText('old')).toBeNull()
  })

  it('keeps an archived channel listed while it is open', () => {
    render(<ChannelList channels={channels} current="c-old" unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} onRename={asyncNoop} />)
    expect(screen.getByText('old')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Archive #old' })).toBeNull()
  })

  it('shows unread counts for other channels and selects by id', () => {
    const onSelect = vi.fn()
    render(<ChannelList channels={channels} current={null} unread={{ 'c-design': 3 }} onSelect={onSelect} onCreate={asyncNoop} onArchive={asyncNoop} onRename={asyncNoop} />)
    expect(screen.getByLabelText('3 unread')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /^design/ }))
    expect(onSelect).toHaveBeenCalledWith('c-design')
  })

  it('archives a channel but never #general', () => {
    const onArchive = vi.fn().mockResolvedValue(undefined)
    render(<ChannelList channels={channels} current="c-design" unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={onArchive} onRename={asyncNoop} />)
    expect(screen.queryByRole('button', { name: 'Archive #general' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Archive #design' }))
    expect(onArchive).toHaveBeenCalledWith('c-design')
  })

  it('creates a channel and reports a rejected name', async () => {
    const onCreate = vi.fn()
      .mockRejectedValueOnce(new Error('channel name taken'))
      .mockResolvedValueOnce(undefined)
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={onCreate} onArchive={asyncNoop} onRename={asyncNoop} />)
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
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} onRename={asyncNoop} />)
    fireEvent.click(screen.getByRole('button', { name: 'New channel' }))
    fireEvent.keyDown(screen.getByLabelText('Channel name'), { key: 'Escape' })
    expect(screen.queryByLabelText('Channel name')).toBeNull()
  })

  it('renames a channel in place, but never #general', async () => {
    const onRename = vi.fn()
      .mockRejectedValueOnce(new Error('#old already exists'))
      .mockResolvedValueOnce(undefined)
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} onRename={onRename} />)
    expect(screen.queryByRole('button', { name: 'Rename #general' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Rename #design' }))
    const input = screen.getByRole('textbox', { name: 'Rename #design' })
    fireEvent.change(input, { target: { value: 'old' } })
    fireEvent.submit(input)
    expect(await screen.findByRole('alert')).toHaveTextContent('#old already exists')
    fireEvent.change(input, { target: { value: '#ui' } })
    fireEvent.submit(input)
    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Rename #design' })).toBeNull())
    expect(onRename).toHaveBeenLastCalledWith('c-design', 'ui')
  })

  it('leaves a rename with Escape or an unchanged name without a call', () => {
    const onRename = vi.fn()
    render(<ChannelList channels={channels} current={null} unread={{}} onSelect={noop} onCreate={asyncNoop} onArchive={asyncNoop} onRename={onRename} />)
    fireEvent.doubleClick(screen.getByRole('button', { name: /^design/ }))
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Rename #design' }), { key: 'Escape' })
    expect(screen.queryByRole('textbox', { name: 'Rename #design' })).toBeNull()
    fireEvent.doubleClick(screen.getByRole('button', { name: /^design/ }))
    fireEvent.submit(screen.getByRole('textbox', { name: 'Rename #design' }))
    expect(screen.queryByRole('textbox', { name: 'Rename #design' })).toBeNull()
    expect(onRename).not.toHaveBeenCalled()
  })
})
