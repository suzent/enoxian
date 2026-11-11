import { render, screen, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import AgentAuthentication from './AgentAuthentication'
import type { AgentAuthState } from '../api'

const state = (status: AgentAuthState['status']): AgentAuthState => ({ status, methods: [], error: null })
describe('agent authentication controls', () => {
  it('distinguishes an installed agent from verified sign-in', () => {
    const onAction = vi.fn().mockResolvedValue(undefined)
    const { rerender } = render(<AgentAuthentication agent="codex" onAction={onAction} />)
    expect(screen.getByText('Not checked')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', {name:'SIGN IN'}))
    expect(onAction).toHaveBeenCalledWith('codex', 'login', undefined)
    rerender(<AgentAuthentication agent="codex" state={state('ready')} onAction={onAction} />)
    expect(screen.getByText('Signed in')).toBeTruthy()
    expect(screen.queryByRole('button', {name:'SIGN IN'})).toBeNull()
    expect(screen.getByRole('button', {name:'SIGN IN AGAIN'})).toBeTruthy()
  })
  it('turns the sign-in button into cancellation during interactive login', () => {
    const onAction = vi.fn().mockResolvedValue(undefined)
    render(<AgentAuthentication agent="claude" state={state('terminal')} onAction={onAction} />)
    fireEvent.click(screen.getByRole('button', {name:'CANCEL'}))
    expect(onAction).toHaveBeenCalledWith('claude', 'cancel')
    expect(screen.queryByRole('button', {name:'SIGN IN'})).toBeNull()
  })
  it('keeps every label in the button so its width does not change', () => {
    render(<AgentAuthentication agent="codex" state={state('auth_required')} onAction={vi.fn()} />)
    const button = screen.getByRole('button', {name:'SIGN IN'})
    expect(button.textContent).toBe('SIGN INSIGN IN AGAINCANCEL')
  })
  it('asks for a check only while none has run', () => {
    const onAutoCheck = vi.fn()
    const { rerender } = render(<AgentAuthentication agent="codex" onAction={vi.fn()} onAutoCheck={onAutoCheck} />)
    expect(onAutoCheck).toHaveBeenCalledWith('codex')
    onAutoCheck.mockClear()
    rerender(<AgentAuthentication agent="codex" state={state('ready')} onAction={vi.fn()} onAutoCheck={onAutoCheck} />)
    expect(onAutoCheck).not.toHaveBeenCalled()
  })
  it('does not present unsupported methods as selectable', () => {
    const auth = {...state('auth_required'), methods:[{id:'unsupported',name:'Unsupported',kind:'other',available:false}]}
    render(<AgentAuthentication agent="custom" state={auth} onAction={vi.fn()} />)
    expect(screen.queryByRole('combobox')).toBeNull()
  })
})
