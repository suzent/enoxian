import { useState, useEffect, useRef, useCallback } from 'react'
import * as THREE from 'three'
import { useApp } from '../context/AppContext'
import { initCircle, enterCircle, leaveCircle, enableCircle, disableCircle } from '../api'
import { makeCircleGeometry, makeShapeParams } from '../lib/circleShape'
import {
  createDitheredComposer,
  addDitherLights,
  makeDitherMaterials,
  type DitheredComposer,
  EXPOSURE_ICON,
  EXPOSURE_TRANSITION_PEAK,
  easeInOut,
} from '../lib/ditherShader'
import type { RitualMode } from './RitualTransition'

interface Props {
  onRitual?: (mode: RitualMode, label?: string) => void
}

type Modal = 'init' | 'enter' | 'leave' | null

interface SceneEntry {
  scene: THREE.Scene
  camera: THREE.PerspectiveCamera
  mesh: THREE.Mesh
  dc: DitheredComposer
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

  // Three.js state — all in refs to avoid re-renders
  const iconRendererRef = useRef<THREE.WebGLRenderer | null>(null)
  const transRendererRef = useRef<THREE.WebGLRenderer | null>(null)
  const transDCRef = useRef<DitheredComposer | null>(null)
  const transSceneRef = useRef(new THREE.Scene())
  const transCameraRef = useRef(new THREE.PerspectiveCamera(60, 1, 0.1, 1000))
  const scenesRef = useRef<Map<string, SceneEntry>>(new Map())
  const canvasMapRef = useRef<Map<string, HTMLCanvasElement>>(new Map())
  const rafRef = useRef(0)
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

    // Full-screen transition renderer + dithered composer
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
    transDC.setExposure(EXPOSURE_TRANSITION_PEAK)
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
      scenesRef.current.forEach(({ mesh, dc }, id) => {
        mesh.rotation.x += mesh.userData.rotX
        mesh.rotation.y += mesh.userData.rotY
        mesh.rotation.z += mesh.userData.rotZ
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
      scenesRef.current.forEach(({ mesh, dc }) => {
        mesh.geometry.dispose()
        ;(mesh.material as THREE.Material).dispose()
        dc.composer.dispose()
      })
      scenesRef.current.clear()
      iconRenderer.dispose()
      document.body.removeChild(iconRenderer.domElement)
      transDC.composer.dispose()
      transRenderer.dispose()
      document.body.removeChild(transRenderer.domElement)
    }
  }, [])

  // Sync icon scenes when circle list changes
  useEffect(() => {
    const scenes = scenesRef.current
    const ids = new Set(circles.map(c => c.circle_id))

    // Remove scenes for circles that no longer exist
    scenes.forEach((entry, id) => {
      if (!ids.has(id)) {
        entry.mesh.geometry.dispose()
        ;(entry.mesh.material as THREE.Material).dispose()
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

      const geo = makeCircleGeometry(circle.circle_name)
      const { flat } = makeDitherMaterials()
      const mesh = new THREE.Mesh(geo, flat)

      const p = makeShapeParams(circle.circle_name)
      mesh.rotation.x = p.initRotX
      mesh.rotation.y = p.initRotY
      mesh.userData.rotX = p.rotX
      mesh.userData.rotY = p.rotY
      mesh.userData.rotZ = p.rotZ

      scene.add(mesh)

      const dc = createDitheredComposer(iconRendererRef.current, scene, camera, 72, 72)
      dc.setExposure(EXPOSURE_ICON)

      scenes.set(circle.circle_id, { scene, camera, mesh, dc })
    })
  }, [circles])

  // Register canvas DOM nodes (called via ref callback on each row)
  const registerCanvas = useCallback((id: string, el: HTMLCanvasElement | null) => {
    if (el) canvasMapRef.current.set(id, el)
    else canvasMapRef.current.delete(id)
  }, [])

  const switchCircle = useCallback(async (targetId: string) => {
    if (animatingRef.current || targetId === activeCircleId) return
    const transRenderer = transRendererRef.current
    const transDC = transDCRef.current
    if (!transRenderer || !transDC) { setActiveCircleId(targetId); return }

    animatingRef.current = true

    const transScene = transSceneRef.current
    const targetCircle = circles.find(c => c.circle_id === targetId)

    // Solid dithered shape for the target circle — expands through the screen during transition
    let transShape: THREE.Mesh | null = null
    if (targetCircle) {
      const geo = makeCircleGeometry(targetCircle.circle_name)
      const { flat } = makeDitherMaterials()
      transShape = new THREE.Mesh(geo, flat)
      transShape.scale.setScalar(0.1)
      const p = makeShapeParams(targetCircle.circle_name)
      transShape.rotation.x = p.initRotX
      transShape.rotation.y = p.initRotY
      transScene.add(transShape)
    }

    transRenderer.domElement.style.display = 'block'

    // Phase 1 — exposure ramps from peak (transparent/white) down to near-zero (dense dots = black)
    // mix-blend-mode:multiply turns dense dither → black screen; sparse dither → transparent
    await new Promise<void>(resolve => {
      const start = performance.now(), dur = 1400
      const LOW = 0.18
      const tick = () => {
        const t = Math.min((performance.now() - start) / dur, 1)
        const e = easeInOut(t)
        transDC.setExposure(EXPOSURE_TRANSITION_PEAK - (EXPOSURE_TRANSITION_PEAK - LOW) * e)
        if (transShape) {
          transShape.scale.setScalar(0.1 + e * 13.9)
          transShape.rotation.x += 0.05
          transShape.rotation.y += 0.07
        }
        transDC.composer.render()
        if (t < 1) requestAnimationFrame(tick); else resolve()
      }
      requestAnimationFrame(tick)
    })

    setActiveCircleId(targetId)

    // Phase 2 — exposure ramps back to peak (dither dissolves → transparent → UI revealed)
    await new Promise<void>(resolve => {
      const start = performance.now(), dur = 1000
      const LOW = 0.18
      const tick = () => {
        const t = Math.min((performance.now() - start) / dur, 1)
        const e = easeInOut(t)
        transDC.setExposure(LOW + (EXPOSURE_TRANSITION_PEAK - LOW) * e)
        if (transShape) {
          transShape.rotation.x += 0.03
          transShape.rotation.y += 0.04
        }
        transDC.composer.render()
        if (t < 1) requestAnimationFrame(tick); else resolve()
      }
      requestAnimationFrame(tick)
    })

    if (transShape) {
      transScene.remove(transShape)
      transShape.geometry.dispose()
      ;(transShape.material as THREE.Material).dispose()
    }
    transDC.setExposure(EXPOSURE_TRANSITION_PEAK)
    transRenderer.domElement.style.display = 'none'
    animatingRef.current = false
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

  const handleLeave = async () => {
    if (!activeCircleId) return
    try {
      await leaveCircle(activeCircleId)
      await reloadCircles()
      setModal(null)
    } catch (err: any) { alert(`Error: ${err.message}`) }
  }

  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

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
                className={`flex items-center gap-2 p-2 border-2 border-obsidian text-left w-full ${
                  isActive ? 'bg-obsidian text-alabaster' : 'bg-alabaster text-obsidian hover:bg-obsidian/5'
                }`}
                style={{ transition: 'none' }}
              >
                <canvas
                  ref={el => registerCanvas(circle.circle_id, el)}
                  width={72} height={72}
                  style={{
                    width: 36, height: 36, flexShrink: 0, display: 'block',
                    imageRendering: 'pixelated',
                    // White dither bg → transparent via multiply on inactive row.
                    // On active (black) row invert the canvas so dither inverts too.
                    filter: isActive ? 'invert(1)' : 'none',
                    mixBlendMode: isActive ? 'normal' : 'multiply',
                  }}
                />
                <div className="flex flex-col min-w-0">
                  <span className="text-[11px] font-bold truncate tracking-wide">
                    {circle.circle_name}
                  </span>
                  {circle.disabled && (
                    <span className="text-[9px] opacity-50 font-bold">VOID</span>
                  )}
                </div>
              </button>
            )
          })}
        </div>

        {activeCircle && (
          <button
            onClick={async () => {
              if (activeCircle.disabled) await enableCircle(activeCircle.circle_id)
              else await disableCircle(activeCircle.circle_id)
              await reloadCircles()
            }}
            className={`border-t-2 border-obsidian px-3 py-2 text-[9px] font-bold tracking-widest w-full flex items-center justify-center gap-2 ${
              !activeCircle.disabled
                ? 'bg-obsidian text-alabaster hover:opacity-80'
                : 'bg-alabaster text-slate border-dashed hover:bg-obsidian/5'
            }`}
            title={activeCircle.disabled
              ? 'Summon the circle into manifest reality'
              : 'Return the circle to the void'}
            style={{ transition: 'none' }}
          >
            {!activeCircle.disabled
              ? <>[█] <span className="tracking-[0.2em]">MANIFEST</span></>
              : <>[{'{∅}'}] <span className="tracking-[0.2em] opacity-60">VOID</span></>
            }
          </button>
        )}

        <div className="border-t-2 border-obsidian flex shrink-0">
          {(['NEW', 'ENTER', 'LEAVE'] as const).map((label, i) => (
            <button
              key={label}
              onClick={() => { setModal((['init', 'enter', 'leave'] as const)[i]); setError('') }}
              className={`flex-1 py-2 text-[9px] font-bold tracking-widest hover:bg-obsidian hover:text-alabaster ${
                i < 2 ? 'border-r-2 border-obsidian' : 'text-slate'
              }`}
              style={{ transition: 'none' }}
            >
              {label}
            </button>
          ))}
        </div>
      </aside>

      {modal && (
        <div className="fixed inset-0 bg-obsidian/60 z-[100] flex items-center justify-center p-4 backdrop-blur-sm">
          <div className="sys-window p-6 w-[400px] max-w-full font-mono text-[11px] uppercase relative">
            <button
              onClick={() => setModal(null)}
              className="absolute top-2 right-3 font-bold text-xl px-1 hover:bg-obsidian hover:text-alabaster"
            >×</button>

            {modal === 'init' && (
              <form onSubmit={handleInit}>
                <h2 className="text-[14px] font-bold mb-4 border-b-2 border-obsidian pb-2">INIT NEW CIRCLE</h2>
                {error && <div className="file-error mb-2">{error}</div>}
                <div className="mb-4">
                  <label className="block text-slate font-bold mb-1 tracking-widest">CIRCLE NAME</label>
                  <input type="text" required value={initName} onChange={e => setInitName(e.target.value)}
                    className="w-full border-2 border-obsidian bg-transparent px-3 py-2 outline-none focus:bg-obsidian/5 font-bold"
                    placeholder="e.g. project-alpha" />
                </div>
                <div className="mb-4">
                  <label className="block text-slate font-bold mb-1 tracking-widest">YOUR NAME <span className="font-normal opacity-40">(OWNER)</span></label>
                  <input type="text" value={initOwner} onChange={e => setInitOwner(e.target.value)}
                    className="w-full border-2 border-obsidian bg-transparent px-3 py-2 outline-none focus:bg-obsidian/5 font-bold"
                    placeholder="e.g. alice" />
                </div>
                <div className="mb-6">
                  <label className="block text-slate font-bold mb-1 tracking-widest">JOIN POLICY</label>
                  <div className="flex gap-2">
                    {(['auto', 'manual'] as const).map(p => (
                      <button key={p} type="button" onClick={() => setInitJoinPolicy(p)}
                        className={`flex-1 py-2 border-2 border-obsidian font-bold ${initJoinPolicy === p ? 'bg-obsidian text-alabaster' : ''}`}>
                        {p.toUpperCase()}
                      </button>
                    ))}
                  </div>
                </div>
                <button type="submit" className="w-full bg-obsidian text-alabaster py-3 font-bold border-2 border-obsidian">
                  CREATE CIRCLE
                </button>
              </form>
            )}

            {modal === 'enter' && (
              <form onSubmit={handleEnter}>
                <h2 className="text-[14px] font-bold mb-4 border-b-2 border-obsidian pb-2">ENTER CIRCLE</h2>
                {error && <div className="file-error mb-2">{error}</div>}
                <div className="mb-4">
                  <label className="block text-slate font-bold mb-1 tracking-widest">INVITE URI</label>
                  <textarea required value={enterTarget}
                    onChange={e => setEnterTarget(e.target.value.trim())}
                    onPaste={e => { e.preventDefault(); setEnterTarget(e.clipboardData.getData('text').trim()) }}
                    className="w-full border-2 border-obsidian bg-transparent px-3 py-2 outline-none focus:bg-obsidian/5 h-24 resize-none font-bold"
                    placeholder="enoxian://..." />
                </div>
                <div className="mb-6">
                  <label className="block text-slate font-bold mb-1 tracking-widest">YOUR NAME <span className="font-normal opacity-40">(OWNER)</span></label>
                  <input type="text" value={enterOwner} onChange={e => setEnterOwner(e.target.value)}
                    className="w-full border-2 border-obsidian bg-transparent px-3 py-2 outline-none focus:bg-obsidian/5 font-bold"
                    placeholder="e.g. bob" />
                </div>
                <button type="submit" className="w-full bg-obsidian text-alabaster py-3 font-bold border-2 border-obsidian">
                  JOIN CIRCLE
                </button>
              </form>
            )}

            {modal === 'leave' && (
              <div>
                <h2 className="text-[14px] font-bold mb-4 border-b-2 border-obsidian pb-2">LEAVE CIRCLE</h2>
                <p className="mb-6 font-bold normal-case text-obsidian/80">
                  Leave <span className="bg-obsidian/10 px-1">"{activeCircle?.circle_name}"</span>?
                  Local config will be removed. Workspace files are untouched.
                </p>
                <div className="flex gap-4">
                  <button onClick={() => setModal(null)} className="flex-1 border-2 border-obsidian py-2 font-bold">CANCEL</button>
                  <button onClick={handleLeave} className="flex-1 bg-obsidian text-alabaster border-2 border-obsidian py-2 font-bold">CONFIRM LEAVE</button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </>
  )
}
