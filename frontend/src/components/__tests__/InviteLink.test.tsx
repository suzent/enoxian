import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import InviteLink from '../InviteLink'

afterEach(cleanup)

describe('InviteLink', () => {
  it('copies the short invite by default and lets users copy the same full invitation', async () => {
    const user = userEvent.setup()
    const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue()
    render(<InviteLink uri="enoxian://s1/short" longUri="enoxian://v2/full" />)
    expect((screen.getByLabelText('Invite link') as HTMLInputElement).value).toBe('enoxian://s1/short')
    await user.click(screen.getByRole('button', { name: 'COPY' }))
    expect(copy).toHaveBeenLastCalledWith('enoxian://s1/short')
    await user.click(screen.getByLabelText('Use full invite'))
    await user.click(screen.getByRole('button', { name: 'COPY' }))
    expect(copy).toHaveBeenLastCalledWith('enoxian://v2/full')
    await user.click(screen.getByLabelText('Use full invite'))
    expect((screen.getByLabelText('Invite link') as HTMLInputElement).value).toBe('enoxian://s1/short')
  })

  it('keeps fallback invites usable and explains why they are full length', async () => {
    const user = userEvent.setup()
    const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue()
    render(<InviteLink uri="enoxian://v2/full" longUri="enoxian://v2/full" note="Relay unavailable; using full invite." />)
    expect(screen.queryByLabelText('Use full invite')).toBeNull()
    expect(screen.getByText('Relay unavailable; using full invite.')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: 'COPY' }))
    expect(copy).toHaveBeenCalledWith('enoxian://v2/full')
  })

  it('supports older API responses and reports clipboard failure without claiming success', async () => {
    const user = userEvent.setup()
    vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue(new Error('denied'))
    render(<InviteLink uri="enoxian://v2/full" />)
    await user.click(screen.getByRole('button', { name: 'COPY' }))
    expect(screen.getByRole('alert').textContent).toContain('copy it manually')
    expect(screen.queryByText('COPIED ✓')).toBeNull()
    expect(screen.queryByLabelText('Use full invite')).toBeNull()
  })
})
