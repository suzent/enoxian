import { describe, it, expect, vi, beforeEach } from 'vitest'

const getCircles = vi.fn()
vi.mock('../../api', () => ({ getCircles }))

const { joinedCircleName } = await import('../circleName')

/**
 * Regression tests for the join caption.
 *
 * The join form asks for an "owner" — the name for *this device* inside the
 * circle — and the animation was handed that instead of the circle's own name,
 * so joining "SUZENT-dev" as "suzy" announced "joining circle suzy".
 */
describe('joinedCircleName', () => {
  beforeEach(() => getCircles.mockReset())

  it('resolves the circle name from the id returned by the join', async () => {
    getCircles.mockResolvedValue([
      { circle_id: 'other', circle_name: 'debug' },
      { circle_id: 'abc', circle_name: 'SUZENT-dev' },
    ])
    await expect(joinedCircleName('abc')).resolves.toBe('SUZENT-dev')
  })

  it('never surfaces a raw id when the circle is not in the list yet', async () => {
    getCircles.mockResolvedValue([{ circle_id: 'other', circle_name: 'debug' }])
    await expect(joinedCircleName('abc')).resolves.toBe('invite')
  })

  it('falls back rather than throwing when the lookup misbehaves', async () => {
    // Anything unusable from the API must produce the caption fallback, never
    // a rejection that would leave the join animation without a caption.
    getCircles.mockResolvedValue(undefined as never)
    await expect(joinedCircleName('abc')).resolves.toBe('invite')
  })

  it('falls back when the join returned no id at all', async () => {
    await expect(joinedCircleName(undefined)).resolves.toBe('invite')
    expect(getCircles).not.toHaveBeenCalled()
  })
})
