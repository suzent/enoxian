import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

const { setActiveCircleId } = vi.hoisted(() => ({ setActiveCircleId: vi.fn() }))
vi.mock('../../context/AppContext', () => ({
  useApp: () => ({
    circles: [
      { circle_id: 'c1', circle_name: 'First', disabled: false },
      { circle_id: 'c2', circle_name: 'Second', disabled: false },
    ],
    activeCircleId: 'c1', setActiveCircleId, reloadCircles: vi.fn(), status: null,
  }),
}))
vi.mock('../../api', () => ({
  getIdentity: vi.fn(async () => ({ user_handle: 'suzy', device_label: 'Mac' })),
  initCircle: vi.fn(), enterCircle: vi.fn(),
}))
vi.mock('../DeviceSettings', () => ({
  default: ({ circleId, onClose }: { circleId?: string; onClose: () => void }) => (
    <div role="dialog" aria-label={circleId ?? 'global'}><button onClick={onClose}>Close settings</button></div>
  ),
}))
import CircleSidebar from '../CircleSidebar'

beforeEach(() => setActiveCircleId.mockClear())

describe('Circle settings entry points', () => {
  it('offers only global settings in the Circle list', async () => {
    render(<CircleSidebar />)
    expect(screen.queryByRole('button', { name: /Settings for/ })).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Open global settings' }))
    expect(screen.getByRole('dialog', { name: 'global' })).toBeTruthy()
    expect(setActiveCircleId).not.toHaveBeenCalled()
  })
})
