import { render, screen, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import AgentAuthentication from './AgentAuthentication'
import type { AgentAuthState } from '../api'

const state = (status: AgentAuthState['status']): AgentAuthState => ({ status, methods: [], error: null })
describe('agent authentication controls', () => {
  it('distinguishes an installed agent from verified sign-in', () => {
    const onAction = vi.fn().mockResolvedValue(undefined)
    const { rerender } = render(<AgentAuthentication agent="codex" onAction={onAction} />)
    expect(screen.getByText('Sign-in not checked')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', {name:'SIGN IN'}))
    expect(onAction).toHaveBeenCalledWith('codex', 'login', undefined)
    rerender(<AgentAuthentication agent="codex" state={state('ready')} onAction={onAction} />)
    expect(screen.getByText('Signed in')).toBeTruthy()
    expect(screen.queryByRole('button', {name:'SIGN IN'})).toBeNull()
    expect(screen.getByRole('button', {name:'SIGN IN AGAIN'})).toBeTruthy()
  })
  it('keeps cancellation available during interactive login', () => {
    const onAction = vi.fn().mockResolvedValue(undefined)
    render(<AgentAuthentication agent="claude" state={state('terminal')} onAction={onAction} />)
    fireEvent.click(screen.getByRole('button', {name:'CANCEL'}))
    expect(onAction).toHaveBeenCalledWith('claude', 'cancel')
    expect((screen.getByRole('button', {name:'SIGN IN'}) as HTMLButtonElement).disabled).toBe(true)
  })
  it('does not present unsupported methods as selectable', () => {
    const auth = {...state('auth_required'), methods:[{id:'unsupported',name:'Unsupported',kind:'other',available:false}]}
    render(<AgentAuthentication agent="custom" state={auth} onAction={vi.fn()} />)
    expect(screen.queryByRole('combobox')).toBeNull()
  })
})
