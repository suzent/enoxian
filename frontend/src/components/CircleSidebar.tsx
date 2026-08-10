import { useState, useEffect, useRef, useCallback } from 'react'
import { Settings } from 'lucide-react'
import { useApp } from '../context/AppContext'
import { initCircle, enterCircle, getIdentity } from '../api'
import { triggerDockBurst } from '../lib/particleEffect'
import DeviceSettings from './DeviceSettings'
import type { RitualMode } from './RitualTransition'

interface Props {
  onRitual?: (mode: RitualMode, label?: string) => void
  ritualCircleName?: string
}

type Modal = 'init' | 'enter' | null

export default function CircleSidebar({ onRitual }: Props) {
  const { circles, activeCircleId, setActiveCircleId, reloadCircles, status } = useApp()

  const [modal, setModal] = useState<Modal>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [initName, setInitName] = useState('')
  const [initOwner, setInitOwner] = useState('')
  const [initJoinPolicy, setInitJoinPolicy] = useState<'auto' | 'manual'>('auto')
  const [enterTarget, setEnterTarget] = useState('')
  const [enterOwner, setEnterOwner] = useState('')
  const [error, setError] = useState('')

  useEffect(() => {
    let cancelled = false
    getIdentity()
      .then(identity => {
        if (cancelled || !identity.user_handle) return
        setInitOwner(owner => owner || identity.user_handle || '')
        setEnterOwner(owner => owner || identity.user_handle || '')
      })
      .catch(() => {})
    return () => { cancelled = true }
  }, [])

  const rowRefMap = useRef<Map<string, HTMLButtonElement>>(new Map())
  const highlightRef = useRef<HTMLDivElement | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)
  const animatingRef = useRef(false)

  // Slide the highlight bar on active change. The glyph confirmation animation
  // is CSS-driven so the selected glyph always remains visible in its row.
  useEffect(() => {
    const hl = highlightRef.current
    const list = listRef.current
    if (!hl || !list) return
    const row = activeCircleId ? rowRefMap.current.get(activeCircleId) : null
    if (!row) { hl.style.opacity = '0'; return }
    const listRect = list.getBoundingClientRect()
    const rowRect = row.getBoundingClientRect()
    hl.style.opacity = '1'
    hl.style.top = `${rowRect.top - listRect.top + list.scrollTop}px`
    hl.style.height = `${rowRect.height}px`
  }, [activeCircleId, circles])

  const activeCircleIdRef = useRef(activeCircleId)
  useEffect(() => { activeCircleIdRef.current = activeCircleId }, [activeCircleId])

  const switchCircle = useCallback((targetId: string) => {
    if (animatingRef.current || targetId === activeCircleIdRef.current) return
    animatingRef.current = true

    const dockEl = document.querySelector('[data-circle-dock]') as HTMLElement | null
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches

    if (!dockEl || reduceMotion) {
      setActiveCircleId(targetId)
      activeCircleIdRef.current = targetId
      if (!reduceMotion) triggerDockBurst()
      animatingRef.current = false
      return
    }

    // Keep the central glyph transition brief; sidebar selection updates after
    // the 100ms exit instead of being locked for the previous half-second.
    const doSwitch = () => {
      setActiveCircleId(targetId)
      activeCircleIdRef.current = targetId
      triggerDockBurst()

      setTimeout(() => {
        dockEl.style.transition = 'none'
        dockEl.style.transform = 'scale(0.86)'
        dockEl.style.opacity = '0'
        requestAnimationFrame(() => {
          dockEl.style.transition =
            'transform 220ms cubic-bezier(0.22,1,0.36,1), opacity 150ms ease-out'
          dockEl.style.transform = 'scale(1)'
          dockEl.style.opacity = '1'
          setTimeout(() => {
            dockEl.style.transition = ''
            dockEl.style.transform = ''
            dockEl.style.opacity = ''
            animatingRef.current = false
          }, 230)
        })
      }, 30)
    }

    dockEl.style.transition = 'transform 100ms ease-in, opacity 90ms ease-in'
    dockEl.style.transform = 'scale(0.82)'
    dockEl.style.opacity = '0'
    setTimeout(doSwitch, 100)
  }, [setActiveCircleId])

  // Modal handlers
  const handleInit = async (e: React.FormEvent) => {
    e.preventDefault(); setError('')
    try {
      const res = await initCircle(initName, initOwner || undefined, initJoinPolicy)
      await reloadCircles()
      if (res.circle_id) setActiveCircleId(res.circle_id)
      setModal(null); onRitual?.('init', initName)
      setInitName(''); setInitOwner(''); setInitJoinPolicy('auto')
    } catch (err: any) { setError(err.message) }
  }

  const handleEnter = async (e: React.FormEvent) => {
    e.preventDefault(); setError('')
    try {
      const res = await enterCircle(enterTarget.trim(), enterOwner || undefined)
      await reloadCircles()
      if (res.circle_id) setActiveCircleId(res.circle_id)
      setModal(null); onRitual?.('enter', enterOwner || 'invite')
      setEnterTarget(''); setEnterOwner('')
    } catch (err: any) { setError(err.message) }
  }

  return (
    <>
      <aside className="app-circles-sidebar sys-window flex flex-col z-10 overflow-hidden font-mono">
        <div className="section-header">
          <span>CIRCLES</span>
          <span className="circle-list-count">{String(circles.length).padStart(2, '0')}</span>
        </div>

        <div ref={listRef} className="circle-list">
          {/* Sliding active highlight — moves between rows via CSS transition */}
          <div ref={highlightRef} className="circle-list-highlight" style={{ opacity: 0 }} />

          {circles.length === 0 && (
            <div className="text-[10px] text-slate p-2">NO CIRCLES</div>
          )}
          {circles.map((circle, index) => {
            const isActive = circle.circle_id === activeCircleId
            return (
              <button
                key={circle.circle_id}
                ref={el => { if (el) rowRefMap.current.set(circle.circle_id, el); else rowRefMap.current.delete(circle.circle_id) }}
                onClick={() => switchCircle(circle.circle_id)}
                className={`circle-row${isActive ? ' circle-row-active' : ''}${circle.disabled ? ' circle-row--void' : ''}`}
                aria-current={isActive ? 'true' : undefined}
              >
                <span className="circle-row__ordinal" aria-hidden="true">
                  {String(index + 1).padStart(2, '0')}
                </span>
                <div className="circle-row__copy">
                  <span className="circle-row__name">{circle.circle_name}</span>
                  <span className={`circle-row__state${isActive ? ' active' : ''}${circle.disabled ? ' void' : ''}`}>
                    {circle.disabled ? 'VOID' : isActive ? 'ACTIVE' : 'CIRCLE'}
                  </span>
                </div>
                <span className="circle-row__marker" aria-hidden="true">→</span>
              </button>
            )
          })}
        </div>

        <div className="border-t-2 border-obsidian flex shrink-0">
          {(['NEW', 'ENTER'] as const).map((label, i) => (
            <button
              key={label}
              onClick={() => { setModal((['init', 'enter'] as const)[i]); setError('') }}
              className={`flex-1 py-1.5 text-[11px] font-bold tracking-widest hover:bg-obsidian hover:text-alabaster ${
                i === 0 ? 'border-r-2 border-obsidian' : ''
              }`}
              style={{ transition: 'none' }}
            >
              {label}
            </button>
          ))}
        </div>

        <button
          className="circle-sidebar-settings"
          onClick={() => setSettingsOpen(true)}
          title="Device settings — agent mention reactions"
          aria-label="Open device settings"
        >
          <Settings size={16} strokeWidth={2.25} aria-hidden="true" />
          <span className="circle-sidebar-settings__identity">
            <strong>{status?.agent_id ?? 'DEVICE SETTINGS'}</strong>
            <small>{status ? 'LOCAL DEVICE · SETTINGS' : 'LOCAL DEVICE'}</small>
          </span>
          <span className="circle-sidebar-settings__arrow" aria-hidden="true">→</span>
        </button>
      </aside>

      {settingsOpen && <DeviceSettings onClose={() => setSettingsOpen(false)} />}

      {modal && (
        <div className="ritual-modal-backdrop">
          <div className="ritual-panel sys-window">
            <button onClick={() => setModal(null)} className="ritual-panel__close" aria-label="Close">×</button>

            {modal === 'init' && (
              <form onSubmit={handleInit} className="ritual-panel__form">
                <div className="ritual-panel__header">CREATE NEW CIRCLE</div>
                <div className="ritual-panel__body">
                  <div className="ritual-divider" />
                  <label className="ritual-field">
                    <span className="ritual-label">CIRCLE NAME</span>
                    <input
                      className="ritual-input"
                      type="text"
                      required
                      value={initName}
                      onChange={e => setInitName(e.target.value)}
                      placeholder="NAME"
                      autoFocus
                    />
                  </label>
                  <div className="ritual-field">
                    <span className="ritual-label">RITUAL POLICY</span>
                    <div className="ritual-segment">
                      {(['auto', 'manual'] as const).map(p => (
                        <button key={p} type="button" onClick={() => setInitJoinPolicy(p)} className={initJoinPolicy === p ? 'active' : ''}>
                          {p.toUpperCase()}
                        </button>
                      ))}
                    </div>
                  </div>
                  {error && <div className="ritual-error">{error}</div>}
                  <div className="ritual-actions">
                    <button type="submit" className="ritual-btn ritual-btn--primary">CREATE</button>
                    <button type="button" onClick={() => setModal(null)} className="ritual-btn ritual-btn--secondary">BACK</button>
                  </div>
                </div>
              </form>
            )}

            {modal === 'enter' && (
              <form onSubmit={handleEnter} className="ritual-panel__form">
                <div className="ritual-panel__header">ENTER THE CIRCLE</div>
                <div className="ritual-panel__body">
                  <div className="ritual-divider" />
                  <label className="ritual-field ritual-field--top">
                    <span className="ritual-label">PACT URI</span>
                    <textarea
                      className="ritual-input ritual-input--textarea ritual-input--uri"
                      required
                      value={enterTarget}
                      onChange={e => setEnterTarget(e.target.value.trim())}
                      onPaste={e => { e.preventDefault(); setEnterTarget(e.clipboardData.getData('text').trim()) }}
                      placeholder="PASTE URI"
                      autoFocus
                    />
                  </label>
                  {error && <div className="ritual-error">{error}</div>}
                  <div className="ritual-actions">
                    <button type="submit" className="ritual-btn ritual-btn--primary">SEAL</button>
                    <button type="button" onClick={() => setModal(null)} className="ritual-btn ritual-btn--secondary">BACK</button>
                  </div>
                </div>
              </form>
            )}
          </div>
        </div>
      )}
    </>
  )
}
