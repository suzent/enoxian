import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import RitualTransition from '../RitualTransition'

const ritual = { mode: 'enter' as const, circleId: 'stable-id', label: 'Design' }
function setup(reduced = false) {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({} as CanvasRenderingContext2D)
  vi.stubGlobal('matchMedia', () => ({ matches: reduced, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 1))
  vi.stubGlobal('cancelAnimationFrame', vi.fn())
}
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals() })
describe('Circle entry', () => {
  it('finishes immediately when reduced motion is requested', () => {
    setup(true)
    const done = vi.fn()
    render(<RitualTransition ritual={ritual} onComplete={done} />)
    expect(done).toHaveBeenCalledOnce()
    expect(requestAnimationFrame).not.toHaveBeenCalled()
  })
  it('allows Escape and cancels pending frames on unmount', () => {
    setup()
    const done = vi.fn()
    const view = render(<RitualTransition ritual={ritual} onComplete={done} />)
    fireEvent.keyDown(window, { key: 'Escape' })
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(done).toHaveBeenCalledOnce()
    view.unmount()
    expect(cancelAnimationFrame).toHaveBeenCalledWith(1)
  })
  it('does not restart when the completion callback changes and supports Skip', () => {
    setup()
    const done = vi.fn()
    const view = render(<RitualTransition ritual={ritual} onComplete={vi.fn()} />)
    view.rerender(<RitualTransition ritual={ritual} onComplete={done} />)
    expect(requestAnimationFrame).toHaveBeenCalledOnce()
    fireEvent.click(screen.getByRole('button', { name: /Skip/ }))
    expect(done).toHaveBeenCalledOnce()
  })
})
