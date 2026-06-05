import { useState, useEffect, useRef, useCallback } from 'react'
import * as THREE from 'three'
import { useApp } from '../context/AppContext'
import { initCircle, enterCircle, getIdentity } from '../api'
import { applyCircleRotation, makeCircleGeometry } from '../lib/circleShape'
import { createDitheredComposer, type DitheredComposer } from '../lib/ditherShader'
import {
  CIRCLE_EXPOSURE,
  createCircleCamera,
  createCircleRenderer,
  prepareCircleScene,
} from '../lib/circleRender'
import { triggerDockBurst } from '../lib/particleEffect'
import type { RitualMode } from './RitualTransition'

interface Props {
  onRitual?: (mode: RitualMode, label?: string) => void
  ritualCircleName?: string
}

type Modal = 'init' | 'enter' | null

interface SceneEntry {
  name: string
  scene: THREE.Scene
  camera: THREE.PerspectiveCamera
  mesh: THREE.Group
  dc: DitheredComposer
  renderer: THREE.WebGLRenderer
}

export default function CircleSidebar({ onRitual, ritualCircleName }: Props) {
  const { circles, activeCircleId, setActiveCircleId, reloadCircles } = useApp()

  const [modal, setModal] = useState<Modal>(null)
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

  const scenesRef = useRef<Map<string, SceneEntry>>(new Map())
  const iconMountMapRef = useRef<Map<string, HTMLDivElement>>(new Map())
  const rowRefMap = useRef<Map<string, HTMLButtonElement>>(new Map())
  const highlightRef = useRef<HTMLDivElement | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)
  const rafRef = useRef(0)
  const animatingRef = useRef(false)
  const prevActiveIdRef = useRef<string | null>(null)

  // rAF loop — animates icons
  useEffect(() => {
    function loop() {
      rafRef.current = requestAnimationFrame(loop)
      const now = performance.now()
      scenesRef.current.forEach(({ name, mesh, dc }) => {
        applyCircleRotation(mesh, name, now)
        dc.composer.render()
      })
    }
    loop()
    return () => {
      cancelAnimationFrame(rafRef.current)
      scenesRef.current.forEach(({ dc, renderer }) => {
        dc.composer.dispose()
        renderer.forceContextLoss()
        renderer.dispose()
        renderer.domElement.remove()
      })
      scenesRef.current.clear()
    }
  }, [])

  // Sync icon scenes when circle list changes
  useEffect(() => {
    const scenes = scenesRef.current
    const ids = new Set(circles.map(c => c.circle_id))
    scenes.forEach((entry, id) => {
      if (!ids.has(id)) {
        entry.dc.composer.dispose()
        entry.renderer.forceContextLoss()
        entry.renderer.dispose()
        entry.renderer.domElement.remove()
        scenes.delete(id)
      }
    })
    circles.forEach(circle => {
      if (scenes.has(circle.circle_id)) return
      const mount = iconMountMapRef.current.get(circle.circle_id)
      if (!mount) return
      createIconScene(circle.circle_id, circle.circle_name, mount, scenes)
    })
  }, [circles])

  // Slide the highlight bar + animate icon mounts on active change
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

    // Pop-out: newly active icon slides right and fades out
    if (activeCircleId) {
      const mount = iconMountMapRef.current.get(activeCircleId)
      if (mount) {
        mount.classList.remove('icon-pop-in')
        mount.classList.add('icon-pop-out')
        mount.addEventListener('animationend', () => mount.classList.remove('icon-pop-out'), { once: true })
      }
    }
    // Pop-in: previously active icon slides back in
    const prev = prevActiveIdRef.current
    if (prev && prev !== activeCircleId) {
      const mount = iconMountMapRef.current.get(prev)
      if (mount) {
        mount.classList.remove('icon-pop-out')
        mount.classList.add('icon-pop-in')
        mount.addEventListener('animationend', () => mount.classList.remove('icon-pop-in'), { once: true })
      }
    }
    prevActiveIdRef.current = activeCircleId
  }, [activeCircleId, circles])

  function createIconScene(
    id: string,
    name: string,
    mount: HTMLDivElement,
    scenes: Map<string, SceneEntry>,
  ) {
    // Render at 88px (dock resolution) so dither density matches dock glyph,
    // CSS-scaled to 36px display size.
    const renderSize = 88
    const displaySize = 36
    const renderer = createCircleRenderer(renderSize, renderSize)
    renderer.domElement.style.cssText =
      `display:block;width:${displaySize}px;height:${displaySize}px;image-rendering:pixelated;`
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    prepareCircleScene(scene)
    const camera = createCircleCamera()
    const group = makeCircleGeometry(name)
    applyCircleRotation(group, name)
    scene.add(group)

    const dc = createDitheredComposer(renderer, scene, camera, renderSize, renderSize)
    dc.setExposure(CIRCLE_EXPOSURE)
    scenes.set(id, { name, scene, camera, mesh: group, dc, renderer })
  }

  const registerIconMount = useCallback((id: string, el: HTMLDivElement | null) => {
    if (!el) { iconMountMapRef.current.delete(id); return }
    iconMountMapRef.current.set(id, el)
    if (scenesRef.current.has(id)) return
    const circle = circles.find(c => c.circle_id === id)
    if (!circle) return
    createIconScene(id, circle.circle_name, el, scenesRef.current)
  }, [circles])

  const activeCircleIdRef = useRef(activeCircleId)
  useEffect(() => { activeCircleIdRef.current = activeCircleId }, [activeCircleId])

  const switchCircle = useCallback((targetId: string) => {
    if (animatingRef.current || targetId === activeCircleIdRef.current) return
    animatingRef.current = true

    const dockEl = document.querySelector('[data-circle-dock]') as HTMLElement | null

    // Pop dock out, switch, pop in
    const doSwitch = () => {
      setActiveCircleId(targetId)
      activeCircleIdRef.current = targetId

      // Fire burst at the dock
      triggerDockBurst()

      // Pop new glyph in after a brief pause for React to swap the glyph
      setTimeout(() => {
        if (dockEl) {
          dockEl.style.transition = 'none'
          dockEl.style.transform = 'scale(0.8)'
          dockEl.style.opacity = '0'
          requestAnimationFrame(() => {
            dockEl.style.transition =
              'transform 300ms cubic-bezier(0.34,1.56,0.64,1), opacity 200ms ease-out'
            dockEl.style.transform = 'scale(1)'
            dockEl.style.opacity = '1'
            setTimeout(() => {
              dockEl.style.transition = ''
              dockEl.style.transform = ''
              dockEl.style.opacity = ''
              animatingRef.current = false
            }, 320)
          })
        } else {
          animatingRef.current = false
        }
      }, 60)
    }

    if (dockEl) {
      dockEl.style.transition = 'transform 140ms ease-in, opacity 120ms ease-in'
      dockEl.style.transform = 'scale(0.7)'
      dockEl.style.opacity = '0'
      setTimeout(doSwitch, 150)
    } else {
      doSwitch()
    }
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
      await enterCircle(enterTarget.trim(), enterOwner || undefined)
      await reloadCircles()
      setModal(null); onRitual?.('enter', enterOwner || 'invite')
      setEnterTarget(''); setEnterOwner('')
    } catch (err: any) { setError(err.message) }
  }

  return (
    <>
      <aside className="app-circles-sidebar sys-window flex flex-col z-10 overflow-hidden font-mono">
        <div className="section-header">
          <span>CIRCLES</span>
        </div>

        <div ref={listRef} className="flex-1 overflow-y-auto flex flex-col gap-1 p-2 min-h-0 relative">
          {/* Sliding active highlight — moves between rows via CSS transition */}
          <div ref={highlightRef} className="circle-list-highlight" style={{ opacity: 0 }} />

          {circles.length === 0 && (
            <div className="text-[10px] text-slate p-2">NO CIRCLES</div>
          )}
          {circles.map(circle => {
            const isActive = circle.circle_id === activeCircleId
            const hideForRitual = circle.circle_name === ritualCircleName
            return (
              <button
                key={circle.circle_id}
                ref={el => { if (el) rowRefMap.current.set(circle.circle_id, el); else rowRefMap.current.delete(circle.circle_id) }}
                onClick={() => switchCircle(circle.circle_id)}
                className={`circle-row flex items-center gap-2 p-2 border-2 border-obsidian text-left w-full ${
                  isActive ? 'circle-row-active bg-alabaster text-obsidian' : 'bg-alabaster text-obsidian hover:bg-obsidian/5'
                }`}
                style={{ transition: 'none', position: 'relative', zIndex: 1 }}
              >
                <div
                  ref={el => registerIconMount(circle.circle_id, el)}
                  className={isActive ? 'circle-icon-mount circle-icon-mount--active' : 'circle-icon-mount'}
                  style={{
                    width: 36, height: 36, flexShrink: 0,
                    mixBlendMode: 'multiply',
                    visibility: hideForRitual ? 'hidden' : 'visible',
                    overflow: 'hidden',
                  }}
                />
                <div className="flex flex-col min-w-0">
                  <span className="font-bold truncate tracking-wide">
                    {circle.circle_name}
                  </span>
                  <span className={`circle-row__state${circle.disabled ? '' : ' inactive'}`}>VOID</span>
                </div>
              </button>
            )
          })}
        </div>

        <div className="border-t-2 border-obsidian flex shrink-0">
          {(['NEW', 'ENTER'] as const).map((label, i) => (
            <button
              key={label}
              onClick={() => { setModal((['init', 'enter'] as const)[i]); setError('') }}
              className={`flex-1 py-2 text-[12px] font-bold tracking-widest hover:bg-obsidian hover:text-alabaster ${
                i === 0 ? 'border-r-2 border-obsidian' : ''
              }`}
              style={{ transition: 'none' }}
            >
              {label}
            </button>
          ))}
        </div>
      </aside>

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
