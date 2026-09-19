import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { circleFamily, CIRCLE_FAMILIES } from '../../lib/circleIdentity'
import CircleActivityButton from '../CircleActivityButton'

vi.mock('../CircleGlyph', () => ({ default: ({ active }: { active: boolean }) => <span data-testid="glyph" data-active={active} /> }))

describe('Circle activity shortcut', () => {
  it('opens details from both pointer and keyboard activation', async () => {
    const onOpen = vi.fn()
    render(<CircleActivityButton name="Design" size={52} working={0} typing={0} onOpen={onOpen} />)
    const button = screen.getByRole('button', { name: 'View activity for Design' })
    await userEvent.click(button)
    await userEvent.keyboard('{Enter}')
    expect(onOpen).toHaveBeenCalledTimes(2)
    expect(screen.getByTestId('glyph')).toHaveAttribute('data-active', 'false')
  })

  it('prioritizes working over typing and stops motion when activity ends or the Circle is disabled', () => {
    const props = { name: 'Design', size: 52, onOpen: vi.fn() }
    const { rerender } = render(<CircleActivityButton {...props} working={2} typing={1} />)
    expect(screen.getByText('2 WORKING')).toBeInTheDocument()
    expect(screen.getByTestId('glyph')).toHaveAttribute('data-active', 'true')
    rerender(<CircleActivityButton {...props} working={0} typing={1} />)
    expect(screen.getByText('1 TYPING')).toBeInTheDocument()
    rerender(<CircleActivityButton {...props} working={0} typing={0} />)
    expect(screen.getByText('Activity')).toBeInTheDocument()
    expect(screen.getByTestId('glyph')).toHaveAttribute('data-active', 'false')
    rerender(<CircleActivityButton {...props} working={2} typing={1} voided />)
    expect(screen.getByText('DISABLED')).toBeInTheDocument()
    expect(screen.getByTestId('glyph')).toHaveAttribute('data-active', 'false')
    expect(screen.getByRole('button')).toBeEnabled()
  })
})

it('assigns a fixed family by Circle ID and ignores old saved choices', () => {
  const expected = circleFamily('stable-id')
  localStorage.setItem('enoxian.circleFamily.stable-id', CIRCLE_FAMILIES.find(f => f !== expected)!)
  expect(circleFamily('stable-id')).toBe(expected)
  const props = { circleId: 'stable-id', name: 'Before', size: 44, working: 0, typing: 0, onOpen: vi.fn() }
  const view = render(<CircleActivityButton {...props} />)
  expect(screen.queryByRole('combobox')).toBeNull()
  view.rerender(<CircleActivityButton {...props} name="After" />)
  expect(screen.getByText('After')).toBeInTheDocument()
  expect(screen.queryByRole('combobox')).toBeNull()
  localStorage.clear()
})

it('announces unread messages without a numeric badge and clears them on activation', async () => {
  const onRead = vi.fn()
  render(<CircleActivityButton name="Design" size={52} working={0} typing={0} unread onRead={onRead} onOpen={vi.fn()} />)
  expect(screen.getByRole('status')).toHaveTextContent('New messages')
  await userEvent.click(screen.getByRole('button'))
  expect(onRead).toHaveBeenCalledOnce()
})
