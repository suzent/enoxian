/**
 * Device identity.
 *
 * The label is not cosmetic: it is the middle segment of every handle that
 * addresses an agent on this machine, and what this device compares an incoming
 * mention against. The tests are mostly about not letting it be changed by
 * accident, or emptied.
 */
import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import DeviceIdentity from '../DeviceIdentity'
import type { IdentityInfo } from '../../api'

const IDENTITY: IdentityInfo = {
  device_label: 'macbook-pro',
  user_handle: 'suzy',
  has_user_key: true,
  update_channel: 'stable',
}

const ADDRESSED = { owner: 'alice', device_label: 'macbook-pro' }

describe('the address it shows', () => {
  it('comes from the Circle, not from the local handle', async () => {
    // The bug this covers: the local handle and the name a Circle knows you by
    // can differ, and showing the local one presents a handle that silently
    // fails when used.
    render(
      <DeviceIdentity identity={IDENTITY} addressedAs={ADDRESSED} circleName="group"
        busy={false} onSave={vi.fn()} />,
    )
    expect(screen.getByText('@alice/macbook-pro/agent')).toBeTruthy()
    expect(screen.queryByText('@suzy/macbook-pro/agent')).toBeNull()
  })

  it('previews a pending device rename, since that segment really does follow', async () => {
    render(
      <DeviceIdentity identity={IDENTITY} addressedAs={ADDRESSED} circleName="group"
        busy={false} onSave={vi.fn()} />,
    )
    const label = screen.getByLabelText('Device name')
    await userEvent.clear(label)
    await userEvent.type(label, 'studio')
    // Device segment follows the edit; the owner segment does not.
    expect(screen.getByText('@alice/studio/agent')).toBeTruthy()
  })

  it('does not pretend a handle edit changes the Circle name', async () => {
    render(
      <DeviceIdentity identity={IDENTITY} addressedAs={ADDRESSED} circleName="group"
        busy={false} onSave={vi.fn()} />,
    )
    await userEvent.clear(screen.getByLabelText('User handle'))
    await userEvent.type(screen.getByLabelText('User handle'), 'zzz')
    expect(screen.getByText('@alice/macbook-pro/agent')).toBeTruthy()
  })

  it('says the handle only applies to Circles joined later', () => {
    render(
      <DeviceIdentity identity={IDENTITY} addressedAs={ADDRESSED} circleName="group"
        busy={false} onSave={vi.fn()} />,
    )
    expect(screen.getByText(/create or join/)).toBeTruthy()
    expect(screen.getByText(/you stay/)).toBeTruthy()
  })

  it('falls back to the local handle when no Circle knows this device yet', () => {
    render(<DeviceIdentity identity={IDENTITY} busy={false} onSave={vi.fn()} />)
    expect(screen.getByText('@suzy/macbook-pro/agent')).toBeTruthy()
  })
})

describe('device identity', () => {

  it('cannot be saved until something changes', async () => {
    const onSave = vi.fn()
    render(<DeviceIdentity identity={IDENTITY} busy={false} onSave={onSave} />)
    expect(screen.getByRole('button', { name: 'SAVE' })).toBeDisabled()

    await userEvent.type(screen.getByLabelText('Device name'), '-2')
    expect(screen.getByRole('button', { name: 'SAVE' })).toBeEnabled()
  })

  it('sends only the field that changed', async () => {
    const onSave = vi.fn(async () => {})
    render(<DeviceIdentity identity={IDENTITY} busy={false} onSave={onSave} />)
    await userEvent.type(screen.getByLabelText('User handle'), 'x')
    await userEvent.click(screen.getByRole('button', { name: 'SAVE' }))
    expect(onSave).toHaveBeenCalledWith({ user_handle: 'suzyx' })
  })

  it('refuses to save an empty device name', async () => {
    // A device with no label cannot be addressed, and cannot recognise a
    // mention as its own.
    const onSave = vi.fn()
    render(<DeviceIdentity identity={IDENTITY} busy={false} onSave={onSave} />)
    await userEvent.clear(screen.getByLabelText('Device name'))
    expect(screen.getByRole('button', { name: 'SAVE' })).toBeDisabled()
  })

  it('allows clearing the handle, which is optional', async () => {
    const onSave = vi.fn(async () => {})
    render(<DeviceIdentity identity={IDENTITY} busy={false} onSave={onSave} />)
    await userEvent.clear(screen.getByLabelText('User handle'))
    await userEvent.click(screen.getByRole('button', { name: 'SAVE' }))
    expect(onSave).toHaveBeenCalledWith({ user_handle: '' })
  })

  it('shows the update channel without offering to change it', () => {
    render(<DeviceIdentity identity={IDENTITY} busy={false} onSave={vi.fn()} />)
    expect(screen.getByText('stable')).toBeTruthy()
    expect(screen.queryByRole('button', { name: /channel/i })).toBeNull()
  })

  it('is inert while a save is in flight', async () => {
    const onSave = vi.fn()
    render(<DeviceIdentity identity={IDENTITY} busy onSave={onSave} />)
    expect(screen.getByLabelText('Device name')).toBeDisabled()
    expect(screen.getByRole('button', { name: 'SAVING…' })).toBeDisabled()
  })
})
