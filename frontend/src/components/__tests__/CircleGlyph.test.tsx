import { act, fireEvent, render } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { drawCircleMark } from '../../lib/circleIdentity'
import CircleGlyph from '../CircleGlyph'
vi.mock('../../lib/circleIdentity', () => ({ circleFamily: () => 'seal', drawCircleMark: vi.fn() }))
let frame: FrameRequestCallback
function setup(reduced = false) {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({ setTransform: vi.fn(), clearRect: vi.fn() } as unknown as CanvasRenderingContext2D)
  vi.stubGlobal('matchMedia', () => ({ matches: reduced, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
  vi.stubGlobal('requestAnimationFrame', vi.fn(cb => { frame = cb; return 1 }))
  vi.stubGlobal('cancelAnimationFrame', vi.fn())
}
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.mocked(drawCircleMark).mockClear() })
it('animates on direct mark hover and settles back after leaving', () => {
  setup()
  const { container } = render(<CircleGlyph name="Circle" circleId="id" size={44} />)
  const canvas = container.querySelector('canvas')!
  act(() => frame(100))
  const radius = vi.mocked(drawCircleMark).mock.lastCall![3]
  fireEvent.pointerEnter(canvas)
  act(() => { frame(132); frame(164); frame(196) })
  expect(vi.mocked(drawCircleMark).mock.lastCall![3]).toBeGreaterThan(radius * 1.1)
  fireEvent.pointerLeave(canvas)
  act(() => { for (let time = 228; time < 1200; time += 32) frame(time) })
  expect(vi.mocked(drawCircleMark).mock.lastCall![3]).toBeCloseTo(radius, 2)
})
it('does not enlarge or rotate on hover with reduced motion enabled', () => {
  setup(true)
  const { container } = render(<CircleGlyph name="Circle" size={44} />)
  fireEvent.pointerEnter(container.querySelector('canvas')!)
  act(() => frame(100))
  const call = vi.mocked(drawCircleMark).mock.lastCall!
  expect(call[3]).toBe(44 / 3.1)
  expect(call[8]).toBe(0)
})
