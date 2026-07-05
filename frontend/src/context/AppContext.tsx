import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react'
import type { Circle, Status } from '../types'
import { getCircles, getStatus } from '../api'

const ACTIVE_CIRCLE_KEY = 'enoxian.activeCircleId'

const loadPersistedCircleId = (): string | null => {
  try { return localStorage.getItem(ACTIVE_CIRCLE_KEY) } catch { return null }
}

interface AppContextValue {
  circles: Circle[]
  circlesLoaded: boolean
  activeCircleId: string | null
  setActiveCircleId: (id: string | null) => void
  status: Status | null
  reloadCircles: () => Promise<void>
}

const AppContext = createContext<AppContextValue>({
  circles: [],
  circlesLoaded: false,
  activeCircleId: null,
  setActiveCircleId: () => {},
  status: null,
  reloadCircles: async () => {},
})

export function AppProvider({ children }: { children: ReactNode }) {
  const [circles, setCircles] = useState<Circle[]>([])
  const [circlesLoaded, setCirclesLoaded] = useState(false)
  const [activeCircleId, setActiveCircleIdState] = useState<string | null>(loadPersistedCircleId)
  const [status, setStatus] = useState<Status | null>(null)

  const reloadCircles = useCallback(async () => {
    try {
      const cs = await getCircles()
      setCircles(cs)
      setCirclesLoaded(true)
      if (activeCircleId && !cs.find(c => c.circle_id === activeCircleId)) {
        const nextCircleId = cs[0]?.circle_id ?? null
        setActiveCircleIdState(nextCircleId)
        setStatus(null)
      } else if (!activeCircleId && cs.length > 0) {
        setActiveCircleIdState(cs[0].circle_id)
      }
    } catch {}
  }, [activeCircleId])

  useEffect(() => {
    reloadCircles()
  }, [])

  // Persist the active circle so a reload restores the last selection instead of
  // jumping back to circles[0]. Covers every mutation path (manual switch and the
  // stale-circle fallback in reloadCircles).
  useEffect(() => {
    try {
      if (activeCircleId) localStorage.setItem(ACTIVE_CIRCLE_KEY, activeCircleId)
      else localStorage.removeItem(ACTIVE_CIRCLE_KEY)
    } catch {}
  }, [activeCircleId])

  const setActiveCircleId = useCallback((id: string | null) => {
    setActiveCircleIdState(id)
    setStatus(null)
  }, [])

  useEffect(() => {
    if (!activeCircleId) {
      setStatus(null)
      return
    }
    let cancelled = false
    getStatus(activeCircleId)
      .then(data => { if (!cancelled) setStatus(data) })
      .catch(() => { if (!cancelled) setStatus(null) })
    return () => { cancelled = true }
  }, [activeCircleId])

  return (
    <AppContext.Provider value={{ circles, circlesLoaded, activeCircleId, setActiveCircleId, status, reloadCircles }}>
      {children}
    </AppContext.Provider>
  )
}

export const useApp = () => useContext(AppContext)
