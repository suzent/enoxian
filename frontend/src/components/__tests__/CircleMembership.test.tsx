import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import CircleMembership from '../CircleMembership'

describe('Circle participation settings', () => {
  it('offers only disable and leave actions for an enabled Circle', async () => {
    const toggle = vi.fn()
    const leave = vi.fn()
    render(<CircleMembership disabled={false} onToggle={toggle} onLeave={leave} />)
    expect(screen.getAllByRole('button')).toHaveLength(2)
    await userEvent.click(screen.getByRole('button', { name: 'Disable Circle' }))
    expect(toggle).toHaveBeenCalledOnce()
    expect(leave).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: 'Leave Circle…' }))
    expect(leave).toHaveBeenCalledOnce()
    expect(screen.getByText(/workspace files are kept/)).toBeTruthy()
  })

  it('offers enable when participation is disabled', async () => {
    const toggle = vi.fn()
    render(<CircleMembership disabled onToggle={toggle} onLeave={vi.fn()} />)
    expect(screen.getByText('Disabled')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Disable Circle' })).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Enable Circle' }))
    expect(toggle).toHaveBeenCalledOnce()
  })
})
