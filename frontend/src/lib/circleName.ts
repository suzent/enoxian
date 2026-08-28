import { getCircles } from '../api'

/**
 * Name of the circle just joined, for the join animation's caption.
 *
 * The join form asks for an owner — the name for *this device* inside the
 * circle — and that is what the caption used to show, so joining "SUZENT-dev"
 * as "suzy" announced "joining circle suzy". The circle's own name is what the
 * caption is for.
 *
 * `/api/enter` returns only an id, and the context's circle list has not
 * re-rendered yet at this point, so look the name up directly. Falls back to
 * the previous generic wording rather than showing an id or an empty caption.
 */
export async function joinedCircleName(circleId?: string): Promise<string> {
  if (!circleId) return 'invite'
  try {
    const circles = await getCircles()
    return circles.find(c => c.circle_id === circleId)?.circle_name || 'invite'
  } catch {
    return 'invite'
  }
}
