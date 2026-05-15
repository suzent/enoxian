import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react'
import type { Circle, Status } from '../types'
import { getCircles, getStatus } from '../api'

interface AppContextValue {
  circles: Circle[]
  activeCircleId: string | null
  setActiveCircleId: (id: string) => void
  status: Status | null
}

const AppContext = createContext<AppContextValue>({
  circles: [],
  activeCircleId: null,
  setActiveCircleId: () => {},
  status: null,
})

export function AppProvider({ children }: { children: ReactNode }) {
  const [circles, setCircles] = useState<Circle[]>([])
  const [activeCircleId, setActiveCircleIdState] = useState<string | null>(null)
  const [status, setStatus] = useState<Status | null>(null)

  useEffect(() => {
    getCircles().then(cs => {
      setCircles(cs)
      if (cs.length > 0 && !activeCircleId) setActiveCircleIdState(cs[0].circle_id)
    }).catch(() => {})
  }, [])

  const setActiveCircleId = useCallback((id: string) => {
    setActiveCircleIdState(id)
    setStatus(null)
  }, [])

  useEffect(() => {
    if (!activeCircleId) return
    getStatus(activeCircleId).then(setStatus).catch(() => {})
  }, [activeCircleId])

  return (
    <AppContext.Provider value={{ circles, activeCircleId, setActiveCircleId, status }}>
      {children}
    </AppContext.Provider>
  )
}

export const useApp = () => useContext(AppContext)
