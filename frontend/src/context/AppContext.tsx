import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react'
import type { Circle, Status } from '../types'
import { getCircles, getStatus } from '../api'

interface AppContextValue {
  circles: Circle[]
  activeCircleId: string | null
  setActiveCircleId: (id: string | null) => void
  status: Status | null
  reloadCircles: () => Promise<void>
}

const AppContext = createContext<AppContextValue>({
  circles: [],
  activeCircleId: null,
  setActiveCircleId: () => {},
  status: null,
  reloadCircles: async () => {},
})

export function AppProvider({ children }: { children: ReactNode }) {
  const [circles, setCircles] = useState<Circle[]>([])
  const [activeCircleId, setActiveCircleIdState] = useState<string | null>(null)
  const [status, setStatus] = useState<Status | null>(null)

  const reloadCircles = useCallback(async () => {
    try {
      const cs = await getCircles()
      setCircles(cs)
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
    <AppContext.Provider value={{ circles, activeCircleId, setActiveCircleId, status, reloadCircles }}>
      {children}
    </AppContext.Provider>
  )
}

export const useApp = () => useContext(AppContext)
