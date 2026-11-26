import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import type { Channel } from '../types'
import { createChannel as apiCreateChannel, getChannels, updateChannel } from '../api'
import { useApp } from './AppContext'

/** Stable key for a channel id; `#general` has id null. */
export const channelKey = (id: string | null | undefined) => id ?? ''

interface ChannelContextValue {
  /** The active circle's channels, `#general` first. Empty until loaded. */
  channels: Channel[]
  /** The channel open in the active circle; null is `#general`. */
  channel: string | null
  /** Messages in channels this viewer has not opened, by `channelKey`. */
  unread: Record<string, number>
  selectChannel: (id: string | null) => void
  createChannel: (name: string, purpose: string) => Promise<void>
  archiveChannel: (id: string) => Promise<void>
  refreshChannels: () => void
  /** Count a message that arrived in a channel other than the open one. */
  noteUnread: (channel: string | null | undefined) => void
}

const ChannelContext = createContext<ChannelContextValue>({
  channels: [],
  channel: null,
  unread: {},
  selectChannel: () => {},
  createChannel: async () => {},
  archiveChannel: async () => {},
  refreshChannels: () => {},
  noteUnread: () => {},
})

/** The active circle's channels and which one is open. The chat panel owns
 *  the chat stream and reports other channels' messages through `noteUnread`;
 *  the sidebar lists the channels and switches between them. */
export function ChannelProvider({ children }: { children: ReactNode }) {
  const { activeCircleId, status } = useApp()
  // The channel open in each circle, kept per circle so switching circles
  // never pairs one circle's channel with another.
  const [channelByCircle, setChannelByCircle] = useState<Record<string, string | null>>({})
  const [channels, setChannels] = useState<Channel[]>([])
  const [unread, setUnread] = useState<Record<string, number>>({})
  const channel = (activeCircleId && channelByCircle[activeCircleId]) || null
  const channelRef = useRef(channel)
  channelRef.current = channel

  const refreshRef = useRef<() => void>(() => {})
  useEffect(() => {
    setChannels([])
    setUnread({})
    if (!activeCircleId) return
    let cancelled = false
    const refresh = () => {
      getChannels(activeCircleId).then(list => { if (!cancelled) setChannels(list) }).catch(() => {})
    }
    refreshRef.current = refresh
    refresh()
    return () => {
      cancelled = true
      refreshRef.current = () => {}
    }
  }, [activeCircleId])

  const refreshChannels = useCallback(() => refreshRef.current(), [])

  const selectChannel = useCallback((id: string | null) => {
    if (!activeCircleId) return
    setChannelByCircle(prev => ({ ...prev, [activeCircleId]: id }))
    setUnread(prev => {
      const key = channelKey(id)
      if (!prev[key]) return prev
      const next = { ...prev }
      delete next[key]
      return next
    })
  }, [activeCircleId])

  const createChannel = useCallback(async (name: string, purpose: string) => {
    if (!activeCircleId) return
    const created = await apiCreateChannel(activeCircleId, name, purpose, status?.agent_id ?? '')
    refreshRef.current()
    selectChannel(created.id)
  }, [activeCircleId, status?.agent_id, selectChannel])

  const archiveChannel = useCallback(async (id: string) => {
    if (!activeCircleId) return
    await updateChannel(activeCircleId, id, { archived: true })
    refreshRef.current()
    if (channelRef.current === id) selectChannel(null)
  }, [activeCircleId, selectChannel])

  const noteUnread = useCallback((id: string | null | undefined) => {
    const key = channelKey(id)
    if (key === channelKey(channelRef.current)) return
    setUnread(prev => ({ ...prev, [key]: (prev[key] ?? 0) + 1 }))
  }, [])

  const value = useMemo(() => ({
    channels, channel, unread, selectChannel, createChannel, archiveChannel, refreshChannels, noteUnread,
  }), [channels, channel, unread, selectChannel, createChannel, archiveChannel, refreshChannels, noteUnread])

  return <ChannelContext.Provider value={value}>{children}</ChannelContext.Provider>
}

export const useChannels = () => useContext(ChannelContext)
