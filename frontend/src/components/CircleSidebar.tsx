import { useState, useEffect, useRef, useCallback } from 'react'
import * as THREE from 'three'
import { useApp } from '../context/AppContext'
import { initCircle, enterCircle, getIdentity } from '../api'
import { applyCircleRotation, makeCircleGeometry } from '../lib/circleShape'
import {
  createDitheredComposer,
  addDitherLights,
  type DitheredComposer,
  EXPOSURE_ICON,
  easeInOut,
} from '../lib/ditherShader'
import type { RitualMode } from './RitualTransition'

interface Props {
  onRitual?: (mode: RitualMode, label?: string) => void
}

const SWITCH_DUR = 720

type Modal = 'init' | 'enter' | null

interface SceneEntry {
  name: string
  scene: THREE.Scene
  camera: THREE.PerspectiveCamera
  mesh: THREE.Mesh
  dc: DitheredComposer
}

interface PlanePose {
  x: number
  y: number
  scale: number
}

export default function CircleSidebar({ onRitual }: Props) {
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

  // Three.js state — all in refs to avoid re-renders
  const iconRendererRef = useRef<THREE.WebGLRenderer | null>(null)
  const scenesRef = useRef<Map<string, SceneEntry>>(new Map())
  const canvasMapRef = useRef<Map<string, HTMLCanvasElement>>(new Map())
  const rafRef = useRef(0)
  // Full-screen transition overlay (re-used across every switch)
  const transRendererRef = useRef<THREE.WebGLRenderer | null>(null)
  const transDCRef = useRef<DitheredComposer | null>(null)
  const transSceneRef = useRef(new THREE.Scene())
  const transCameraRef = useRef(new THREE.PerspectiveCamera(60, 1, 0.1, 1000))
  const animatingRef = useRef(false)

  // Bootstrap Three.js renderers once
  useEffect(() => {
    // Shared icon renderer (white bg — mix-blend-mode:multiply makes white transparent)
    const iconRenderer = new THREE.WebGLRenderer({ antialias: false })
    iconRenderer.setPixelRatio(1)
    iconRenderer.setClearColor(0xffffff, 1)
    iconRenderer.setSize(72, 72)
    iconRenderer.domElement.style.cssText = 'position:fixed;top:-9999px;left:-9999px;pointer-events:none;'
    document.body.appendChild(iconRenderer.domElement)
    iconRendererRef.current = iconRenderer

    // Full-screen transition renderer + dithered composer (white bg → transparent
    // via mix-blend-mode:multiply; the dithered shape shows through over the UI).
    const transRenderer = new THREE.WebGLRenderer({ antialias: false })
    transRenderer.setSize(window.innerWidth, window.innerHeight)
    transRenderer.setPixelRatio(1)
    transRenderer.setClearColor(0xffffff, 1)
    transRenderer.domElement.style.cssText =
      'position:fixed;inset:0;z-index:1000;pointer-events:none;display:none;mix-blend-mode:multiply;'
    document.body.appendChild(transRenderer.domElement)
    transRendererRef.current = transRenderer

    const transScene = transSceneRef.current
    transScene.background = new THREE.Color(0xffffff)
    addDitherLights(transScene)

    const transCamera = transCameraRef.current
    transCamera.position.z = 5
    transCamera.aspect = window.innerWidth / window.innerHeight
    transCamera.updateProjectionMatrix()

    const transDC = createDitheredComposer(
      transRenderer, transScene, transCamera,
      window.innerWidth, window.innerHeight,
    )
    transDC.setExposure(EXPOSURE_ICON)
    transDCRef.current = transDC

    const onResize = () => {
      const W = window.innerWidth, H = window.innerHeight
      transRenderer.setSize(W, H)
      transDC.setSize(W, H)
      transCamera.aspect = W / H
      transCamera.updateProjectionMatrix()
    }
    window.addEventListener('resize', onResize)

    // Single rAF loop: render each icon scene via its own dithered composer → blit
    function loop() {
      rafRef.current = requestAnimationFrame(loop)
      const now = performance.now()
      scenesRef.current.forEach(({ name, mesh, dc }, id) => {
        applyCircleRotation(mesh, name, now)
        dc.composer.render()
        const canvas = canvasMapRef.current.get(id)
        if (canvas) {
          const ctx = canvas.getContext('2d')
          if (ctx) {
            ctx.clearRect(0, 0, canvas.width, canvas.height)
            ctx.drawImage(iconRenderer.domElement, 0, 0, canvas.width, canvas.height)
          }
        }
      })
    }
    loop()

    return () => {
      cancelAnimationFrame(rafRef.current)
      window.removeEventListener('resize', onResize)
      scenesRef.current.forEach(({ dc }) => {
        // Skip geometry/material dispose since mesh is now a Group
        dc.composer.dispose()
      })
      scenesRef.current.clear()
      iconRenderer.forceContextLoss()
      iconRenderer.dispose()
      if (iconRenderer.domElement.parentNode) {
        iconRenderer.domElement.parentNode.removeChild(iconRenderer.domElement)
      }
      transDC.composer.dispose()
      transRenderer.forceContextLoss()
      transRenderer.dispose()
      if (transRenderer.domElement.parentNode) {
        transRenderer.domElement.parentNode.removeChild(transRenderer.domElement)
      }
    }
  }, [])

  // Sync icon scenes when circle list changes
  useEffect(() => {
    const scenes = scenesRef.current
    const ids = new Set(circles.map(c => c.circle_id))

    // Remove scenes for circles that no longer exist
    scenes.forEach((_, id) => {
      if (!ids.has(id)) {
        // Skip simple dispose since it's a Group
        scenes.delete(id)
      }
    })

    // Create scenes for new circles
    circles.forEach(circle => {
      if (scenes.has(circle.circle_id) || !iconRendererRef.current) return
      const scene = new THREE.Scene()
      scene.background = new THREE.Color(0xffffff)
      addDitherLights(scene)

      const camera = new THREE.PerspectiveCamera(50, 1, 0.1, 100)
      camera.position.z = 2.8

      const group = makeCircleGeometry(circle.circle_name)

      applyCircleRotation(group, circle.circle_name)

      scene.add(group)

      const dc = createDitheredComposer(iconRendererRef.current, scene, camera, 72, 72)
      dc.setExposure(EXPOSURE_ICON)

      scenes.set(circle.circle_id, { name: circle.circle_name, scene, camera, mesh: group as unknown as THREE.Mesh, dc })
    })
  }, [circles])

  // Register canvas DOM nodes (called via ref callback on each row)
  const registerCanvas = useCallback((id: string, el: HTMLCanvasElement | null) => {
    if (el) canvasMapRef.current.set(id, el)
    else canvasMapRef.current.delete(id)
  }, [])

  const cloneTransitionShape = (entry: SceneEntry) => {
    const shape = entry.mesh.clone()
    shape.traverse((child) => {
      if ((child as THREE.Mesh).isMesh) {
        const mesh = child as THREE.Mesh
        if (mesh.material) {
          mesh.material = (mesh.material as THREE.Material).clone()
        }
      }
    })
    return shape
  }

  // Use a ref to hold the absolute latest activeCircleId to avoid stale closures
  // during fast switching. AppContext provides `activeCircleId` but useCallback 
  // without it in the dependency array will capture an old closure.
  const activeCircleIdRef = useRef(activeCircleId)
  useEffect(() => {
    activeCircleIdRef.current = activeCircleId
  }, [activeCircleId])

  const switchCircle = useCallback((targetId: string) => {
    const currentActiveId = activeCircleIdRef.current
    if (animatingRef.current || targetId === currentActiveId) return
    const transRenderer = transRendererRef.current
    const transDC = transDCRef.current
    const targetEntry = scenesRef.current.get(targetId)
    const currentEntry = currentActiveId ? scenesRef.current.get(currentActiveId) : null
    const targetCanvas = canvasMapRef.current.get(targetId)
    const currentCanvas = currentActiveId ? canvasMapRef.current.get(currentActiveId) : null
    
    if (!transRenderer || !transDC || !targetEntry || !targetCanvas) { 
      setActiveCircleId(targetId);
      return 
    }
    
    animatingRef.current = true

    const transScene = transSceneRef.current
    const transCamera = transCameraRef.current

    const visH = 2 * Math.tan((transCamera.fov * Math.PI) / 180 / 2) * transCamera.position.z
    const visW = visH * transCamera.aspect
    const targetRect = targetCanvas.getBoundingClientRect()
    const currentRect = currentCanvas?.getBoundingClientRect()
    const dock = document.querySelector('[data-circle-dock] canvas') ?? document.querySelector('[data-circle-dock]')
    const dockRect = dock?.getBoundingClientRect()
    if (!dockRect) {
      setActiveCircleId(targetId)
      animatingRef.current = false
      return
    }

    const toPlane = (r: DOMRect): PlanePose => {
      const cx = r.left + r.width / 2
      const cy = r.top + r.height / 2
      return {
        x: ((cx / window.innerWidth) * 2 - 1) * (visW / 2),
        y: -((cy / window.innerHeight) * 2 - 1) * (visH / 2),
        scale: (r.height * 0.8) / (2 * (window.innerHeight / visH)),
      }
    }
    const dockPose = toPlane(dockRect)
    const targetPose = toPlane(targetRect)
    const currentPose = currentRect ? toPlane(currentRect) : targetPose

    const outgoingShape = currentEntry ? cloneTransitionShape(currentEntry) : null
    const incomingShape = cloneTransitionShape(targetEntry)

    if (outgoingShape) {
      outgoingShape.position.set(dockPose.x, dockPose.y, 0)
      outgoingShape.scale.setScalar(dockPose.scale)
      transScene.add(outgoingShape)
    }
    incomingShape.position.set(targetPose.x, targetPose.y, 0)
    incomingShape.scale.setScalar(targetPose.scale)
    transScene.add(incomingShape)

    targetCanvas.style.visibility = 'hidden'
    if (currentCanvas) currentCanvas.style.visibility = 'hidden'
    if (dock instanceof HTMLElement) dock.style.visibility = 'hidden'

    transDC.setExposure(EXPOSURE_ICON)
    transRenderer.domElement.style.display = 'block'

    const start = performance.now()
    const tick = () => {
      const now = performance.now()
      const p = easeInOut(Math.min((now - start) / SWITCH_DUR, 1))
      incomingShape.position.set(
        targetPose.x + (dockPose.x - targetPose.x) * p,
        targetPose.y + (dockPose.y - targetPose.y) * p,
        0,
      )
      incomingShape.scale.setScalar(targetPose.scale + (dockPose.scale - targetPose.scale) * p)
      applyCircleRotation(incomingShape, targetEntry.name, now)

      if (outgoingShape) {
        outgoingShape.position.set(
          dockPose.x + (currentPose.x - dockPose.x) * p,
          dockPose.y + (currentPose.y - dockPose.y) * p,
          0,
        )
        outgoingShape.scale.setScalar(dockPose.scale + (currentPose.scale - dockPose.scale) * p)
        if (currentEntry) applyCircleRotation(outgoingShape, currentEntry.name, now)
      }
      transDC.composer.render()
      if (p < 1) {
        requestAnimationFrame(tick)
      } else {
        setActiveCircleId(targetId)
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            transScene.remove(incomingShape)
            if (outgoingShape) transScene.remove(outgoingShape)
            targetCanvas.style.visibility = ''
            if (currentCanvas) currentCanvas.style.visibility = ''
            if (dock instanceof HTMLElement) dock.style.visibility = ''
            transRenderer.domElement.style.display = 'none'
            transDC.setExposure(EXPOSURE_ICON)
            animatingRef.current = false
          })
        })
      }
    }
    requestAnimationFrame(tick)
  }, [activeCircleId, setActiveCircleId, circles])

  // Modal handlers
  const handleInit = async (e: React.FormEvent) => {
    e.preventDefault(); setError('')
    try {
      const res = await initCircle(initName, initOwner || undefined, initJoinPolicy)
      await reloadCircles()
      if (res.circle_id) switchCircle(res.circle_id)
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

        <div className="flex-1 overflow-y-auto flex flex-col gap-1 p-2 min-h-0">
          {circles.length === 0 && (
            <div className="text-[10px] text-slate p-2">NO CIRCLES</div>
          )}
          {circles.map(circle => {
            const isActive = circle.circle_id === activeCircleId
            return (
              <button
                key={circle.circle_id}
                onClick={() => switchCircle(circle.circle_id)}
                className={`circle-row flex items-center gap-2 p-2 border-2 border-obsidian text-left w-full ${
                  isActive ? 'circle-row-active bg-alabaster text-obsidian' : 'bg-alabaster text-obsidian hover:bg-obsidian/5'
                }`}
                style={{ transition: 'none' }}
              >
                <canvas
                  ref={el => registerCanvas(circle.circle_id, el)}
                  width={72} height={72}
                  style={{
                    width: 36, height: 36, flexShrink: 0, display: 'block',
                    imageRendering: 'pixelated',
                    mixBlendMode: 'multiply',
                    opacity: isActive ? 0 : 1,
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
            <button
              onClick={() => setModal(null)}
              className="ritual-panel__close"
              aria-label="Close"
            >×</button>

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
                        <button
                          key={p}
                          type="button"
                          onClick={() => setInitJoinPolicy(p)}
                          className={initJoinPolicy === p ? 'active' : ''}
                        >
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
