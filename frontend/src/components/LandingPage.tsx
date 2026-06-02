/**
 * LandingPage — first-run screen shown when the user has no circles.
 *
 * Combines:
 *  - A full-screen Three.js cyber-angel scene (dithered, mix-blend-mode:multiply)
 *  - A centered sys-window overlay for identity setup + circle init/enter
 */
import { useEffect, useRef, useState, useCallback } from 'react'
import * as THREE from 'three'
import {
  createDitheredComposer,
  addDitherLights,
} from '../lib/ditherShader'
import {
  getIdentity,
  setIdentity,
  linkDevice,
  initCircle,
  enterCircle,
  createUserIdentity,
} from '../api'
import { useApp } from '../context/AppContext'

// ── Types ─────────────────────────────────────────────────────────────────────

interface Props {
  onEntered: () => void
}

type UIState = 'setup' | 'init-form' | 'enter-form' | 'mnemonic-backup'
type AnimState = 'idle' | 'erupting' | 'breathing' | 'revelation' | 'ingress' | 'fading' | 'done'

// ── Angel scene (imperative handle passed out of useEffect) ───────────────────

interface AngelScene {
  triggerEruption(onComplete: () => void): void
  dispose(): void
}

function buildAngelScene(mount: HTMLDivElement): AngelScene {
  const W = window.innerWidth
  const H = window.innerHeight

  // Renderer
  const renderer = new THREE.WebGLRenderer({ antialias: false })
  renderer.setSize(W, H)
  renderer.setPixelRatio(1)
  renderer.setClearColor(0xffffff, 1)
  mount.appendChild(renderer.domElement)

  // Scene + camera
  const scene = new THREE.Scene()
  scene.background = new THREE.Color(0xffffff)
  const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 150)
  camera.position.set(0, 0, 32)

  addDitherLights(scene)

  // Material hierarchy from the reference demo — different luminance per part
  // so the dither creates clear visual separation between monolith / blades / halos.
  const solidMat = new THREE.MeshPhongMaterial({ color: 0x1a1a1a, flatShading: true })  // very dark — dense dither on monolith
  const bladeMat = new THREE.MeshPhongMaterial({ color: 0x666666, flatShading: true, shininess: 20 }) // mid — medium dither on blades
  const ringMat  = new THREE.MeshPhongMaterial({ color: 0xcccccc, flatShading: true })  // light — sparse dither on halos

  // ── Halos (hidden at start, eruption reveals them) ────────────────────────
  const halos = new THREE.Group()
  halos.position.z = -8
  halos.scale.set(0, 0, 0)
  scene.add(halos)

  const halo1 = new THREE.Mesh(new THREE.TorusGeometry(12, 0.2, 4, 64), ringMat)
  halos.add(halo1)

  const halo2 = new THREE.Mesh(new THREE.TorusGeometry(14, 0.4, 4, 64), ringMat)
  halo2.rotation.x = Math.PI / 2
  halos.add(halo2)

  // ── Center monolith ───────────────────────────────────────────────────────
  const monolithGroup = new THREE.Group()
  scene.add(monolithGroup)

  const monoShapeLeft = new THREE.Shape()
  monoShapeLeft.moveTo(0, 10)
  monoShapeLeft.lineTo(-1.8, 5)
  monoShapeLeft.lineTo(-1.8, -5)
  monoShapeLeft.lineTo(0, -9)
  monoShapeLeft.lineTo(0, 10)

  const monoShapeRight = new THREE.Shape()
  monoShapeRight.moveTo(0, 10)
  monoShapeRight.lineTo(1.8, 5)
  monoShapeRight.lineTo(1.8, -5)
  monoShapeRight.lineTo(0, -9)
  monoShapeRight.lineTo(0, 10)

  const extrudeSettings = {
    depth: 1.5,
    bevelEnabled: true,
    bevelThickness: 0.1,
    bevelSize: 0.1,
    bevelSegments: 1,
  }

  const geoLeft = new THREE.ExtrudeGeometry(monoShapeLeft, extrudeSettings)
  geoLeft.center()
  const meshLeft = new THREE.Mesh(geoLeft, solidMat)
  meshLeft.position.set(-0.95, 0.5, 0)
  monolithGroup.add(meshLeft)

  const geoRight = new THREE.ExtrudeGeometry(monoShapeRight, extrudeSettings)
  geoRight.center()
  const meshRight = new THREE.Mesh(geoRight, solidMat)
  meshRight.position.set(0.95, 0.5, 0)
  monolithGroup.add(meshRight)

  // Node rings on the monolith
  const nodesGroup = new THREE.Group()
  const nodeGeo = new THREE.TorusGeometry(0.3, 0.1, 4, 16)
  const squareGeo = new THREE.BoxGeometry(0.7, 0.7, 0.2)

  const node1 = new THREE.Mesh(nodeGeo, solidMat)
  node1.position.set(0, 3, 0.8)
  nodesGroup.add(node1)
  const node2 = new THREE.Mesh(squareGeo, solidMat)
  node2.position.set(0, -1, 0.8)
  nodesGroup.add(node2)
  const node3 = new THREE.Mesh(nodeGeo, solidMat)
  node3.position.set(0, -5, 0.8)
  nodesGroup.add(node3)
  monolithGroup.add(nodesGroup)

  // ── Wings ─────────────────────────────────────────────────────────────────
  interface WingData {
    group: THREE.Group
    feathers: THREE.Mesh[]
  }

  function createMassiveCyberWing(isLeft: boolean): WingData {
    const group = new THREE.Group()
    const dir = isLeft ? -1 : 1

    // Circuit arm base
    const armGroup = new THREE.Group()
    const traceGeoH = new THREE.BoxGeometry(2, 0.4, 0.8)
    const traceGeoD = new THREE.BoxGeometry(3, 0.4, 0.8)

    const trace1 = new THREE.Mesh(traceGeoH, solidMat)
    trace1.position.set(dir * 1, -4, 0)
    armGroup.add(trace1)

    const trace2 = new THREE.Mesh(traceGeoD, solidMat)
    trace2.rotation.z = dir * Math.PI / 4
    trace2.position.set(dir * 2.8, -3.1, 0)
    armGroup.add(trace2)

    const trace3 = new THREE.Mesh(traceGeoH, solidMat)
    trace3.position.set(dir * 4.6, -2.0, 0)
    armGroup.add(trace3)

    group.add(armGroup)

    // Feather blades — 15 per side
    const feathersGroup = new THREE.Group()
    const feathers: THREE.Mesh[] = []
    const numBlades = 15

    // Shared blade geometry: long sharp diamond — pivot at base
    const bladeGeo = new THREE.CylinderGeometry(0, 1.2, 1, 4)
    bladeGeo.rotateX(Math.PI / 4)
    bladeGeo.translate(0, 0.5, 0)

    for (let i = 0; i < numBlades; i++) {
      const t = i / (numBlades - 1) // 0.0 → 1.0

      const blade = new THREE.Mesh(bladeGeo, bladeMat)

      const length = 18 - t * 12
      const width = 0.8 - t * 0.4

      const posX = dir * (4.5 + t * 6)
      const posY = -2.0 + t * 12
      const posZ = -t * 8

      const rotZ = dir * (-0.8 - t * 1.5)
      const rotY = dir * (t * 0.5)
      const rotX = t * 0.5

      blade.userData = {
        tPos: new THREE.Vector3(posX, posY, posZ),
        tRot: new THREE.Euler(rotX, rotY, rotZ),
        tScale: new THREE.Vector3(width, length, 0.3),
      }

      // Initial folded state: tucked into the circuit arm
      blade.scale.set(width, 0, 0.3)
      blade.position.set(dir * 4.5, -2.0, 0)
      blade.rotation.set(0, 0, dir * -0.8)

      feathers.push(blade)
      feathersGroup.add(blade)
    }

    group.add(feathersGroup)

    // Hidden placement until eruption
    group.position.x = dir * -4
    group.position.z = -2

    return { group, feathers }
  }

  const wingL = createMassiveCyberWing(true)
  const wingR = createMassiveCyberWing(false)
  scene.add(wingL.group)
  scene.add(wingR.group)

  // ── Dithered post-processing ──────────────────────────────────────────────
  const dc = createDitheredComposer(renderer, scene, camera, W, H)

  // ── Animation state machine ───────────────────────────────────────────────
  // Full sequence (matches reference demo timeline):
  //   idle → erupting (2.5s) → breathing (2s) → revelation (0.6s)
  //        → ingress (3s) → fading (1s) → done → onComplete()
  let animState: AnimState = 'idle'
  let phaseStart = 0
  let baseTime = performance.now()
  let rafId = 0
  let completionCallback: (() => void) | null = null

  // Store initial blade rotations for the ingress fold
  const bladeFoldStart: THREE.Euler[] = []

  // Per-blade eruption cache to avoid userData lookups every frame
  interface BladeTarget {
    blade: THREE.Mesh
    initPos: THREE.Vector3
    initRot: THREE.Euler
    initScale: THREE.Vector3
    delay: number // 0..1 normalized start within eruption
  }

  const bladeTargets: BladeTarget[] = []
  const allBlades = [...wingL.feathers, ...wingR.feathers]
  allBlades.forEach((blade, i) => {
    bladeTargets.push({
      blade,
      initPos: blade.position.clone(),
      initRot: new THREE.Euler(blade.rotation.x, blade.rotation.y, blade.rotation.z),
      initScale: blade.scale.clone(),
      delay: (i % 15) * 0.04, // cascade: 0..0.56 s normalized
    })
  })

  // Simple easing helpers (no external deps)
  function easeOutExpo(t: number): number {
    return t >= 1 ? 1 : 1 - Math.pow(2, -10 * t)
  }
  function easeOutBack(t: number): number {
    const c1 = 1.70158
    const c3 = c1 + 1
    return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2)
  }
  function clamp01(v: number): number {
    return Math.max(0, Math.min(1, v))
  }
  function lerp(a: number, b: number, t: number): number {
    return a + (b - a) * t
  }

  function tick() {
    rafId = requestAnimationFrame(tick)
    const now = performance.now()
    const elapsed = now - baseTime
    const t = elapsed * 0.001 // seconds

    if (animState === 'idle') {
      // Monolith gentle bob
      monolithGroup.position.y = Math.sin(t) * 0.3
      // Camera slight breathing
      camera.position.y = Math.sin(t * 0.4) * 0.15
    } else if (animState === 'erupting') {
      const et = clamp01((now - eruptionStart) / ERUPTION_DURATION) // 0→1

      // Wings slide out: group.position.x → 0
      wingL.group.position.x = lerp(-4, 0, easeOutExpo(clamp01(et / 0.5)))
      wingR.group.position.x = lerp(4, 0, easeOutExpo(clamp01(et / 0.5)))

      // Halos scale 0 → 1
      const haloT = easeOutExpo(clamp01((et - 0.1) / 0.8))
      halos.scale.set(haloT, haloT, haloT)

      // Halo rotation once visible
      halo1.rotation.z -= 0.005
      halo2.rotation.y += 0.008
      halo2.rotation.z += 0.005

      // Camera pull back slightly
      camera.position.z = lerp(32, 34, easeOutExpo(clamp01(et / 0.7)))
      camera.position.y = 0

      // Monolith stop bobbing (freeze at neutral)
      monolithGroup.position.y = lerp(monolithGroup.position.y, 0, 0.1)

      // Individual blade eruption with cascade delay
      for (const bt of bladeTargets) {
        const localStart = bt.delay
        const localDuration = 0.5 // each blade takes 50% of duration to fully deploy
        const localT = clamp01((et - localStart) / localDuration)
        if (localT <= 0) continue

        const ease = easeOutBack(localT)
        const easePos = easeOutExpo(localT)

        bt.blade.scale.set(
          lerp(bt.initScale.x, bt.blade.userData.tScale.x, ease),
          lerp(bt.initScale.y, bt.blade.userData.tScale.y, ease),
          lerp(bt.initScale.z, bt.blade.userData.tScale.z, ease),
        )
        bt.blade.position.set(
          lerp(bt.initPos.x, bt.blade.userData.tPos.x, easePos),
          lerp(bt.initPos.y, bt.blade.userData.tPos.y, easePos),
          lerp(bt.initPos.z, bt.blade.userData.tPos.z, easePos),
        )
        bt.blade.rotation.set(
          lerp(bt.initRot.x, bt.blade.userData.tRot.x, easePos),
          lerp(bt.initRot.y, bt.blade.userData.tRot.y, easePos),
          lerp(bt.initRot.z, bt.blade.userData.tRot.z, easePos),
        )
      }

      // Blades breathing once fully extended
      if (et > 0.8) {
        const breathT = now * 0.002
        wingL.feathers.forEach((blade, i) => {
          blade.rotation.x = blade.userData.tRot.x + Math.sin(breathT + i * 0.1) * 0.05
        })
        wingR.feathers.forEach((blade, i) => {
          blade.rotation.x = blade.userData.tRot.x + Math.sin(breathT + i * 0.1) * 0.05
        })
      }

      if (et >= 1) {
        animState = 'done'
      }
    } else {
      // done — keep halos rotating
      halo1.rotation.z -= 0.005
      halo2.rotation.y += 0.008
      halo2.rotation.z += 0.005
    }

    dc.composer.render()
  }

  tick()

  // Resize handler
  function onResize() {
    const nW = window.innerWidth
    const nH = window.innerHeight
    renderer.setSize(nW, nH)
    dc.setSize(nW, nH)
    camera.aspect = nW / nH
    camera.updateProjectionMatrix()
  }
  window.addEventListener('resize', onResize)

  return {
    triggerEruption() {
      if (animState !== 'idle') return
      animState = 'erupting'
      eruptionStart = performance.now()
    },
    dispose() {
      cancelAnimationFrame(rafId)
      window.removeEventListener('resize', onResize)
      if (mount.contains(renderer.domElement)) mount.removeChild(renderer.domElement)
      renderer.dispose()
    },
  }
}

// ── Component ─────────────────────────────────────────────────────────────────

export default function LandingPage({ onEntered }: Props) {
  const mountRef = useRef<HTMLDivElement>(null)
  const angelRef = useRef<AngelScene | null>(null)
  const { reloadCircles } = useApp()

  // UI state
  const [uiState, setUIState] = useState<UIState>('setup')
  const [linkExpanded, setLinkExpanded] = useState(false)

  // Identity fields
  const [userName, setUserName] = useState('')
  const [deviceLabel, setDeviceLabel] = useState('')
  const [hasUserKey, setHasUserKey] = useState(false)
  const [isEditingIdentity, setIsEditingIdentity] = useState(false)

  // Init-circle form
  const [circleName, setCircleName] = useState('')
  const [joinPolicy, setJoinPolicy] = useState<'auto' | 'manual'>('auto')

  // Enter-circle form
  const [inviteUri, setInviteUri] = useState('')

  // Link-device form
  const [linkHandle, setLinkHandle] = useState('')
  const [linkMnemonic, setLinkMnemonic] = useState('')
  const [linkSuccess, setLinkSuccess] = useState(false)

  // Mnemonic backup
  const [mnemonic, setMnemonic] = useState('')

  // Error / loading / eruption
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [isErupting, setIsErupting] = useState(false)

  // Load identity on mount
  useEffect(() => {
    getIdentity().then(info => {
      if (info.user_handle) setUserName(info.user_handle)
      setDeviceLabel(info.device_label)
      setHasUserKey(info.has_user_key)
      // If identity not yet configured, start in edit mode
      if (!info.user_handle) setIsEditingIdentity(true)
    }).catch(() => { setIsEditingIdentity(true) })
  }, [])

  // Build Three.js scene on mount
  useEffect(() => {
    const mount = mountRef.current
    if (!mount) return
    const angel = buildAngelScene(mount)
    angelRef.current = angel
    return () => {
      angel.dispose()
      angelRef.current = null
    }
  }, [])

  // ── Helpers ─────────────────────────────────────────────────────────────

  async function saveIdentityIfNeeded() {
    try {
      await setIdentity({
        user_handle: userName.trim() || undefined,
        device_label: deviceLabel.trim() || undefined,
      })
    } catch {
      // non-fatal — proceed
    }
  }

  function triggerEruptionAndComplete() {
    setIsErupting(true)          // hide the overlay box immediately
    angelRef.current?.triggerEruption()
    setTimeout(async () => {
      await reloadCircles()
      onEntered()
    }, 2600)
  }

  // ── Actions ──────────────────────────────────────────────────────────────

  const handleInitClick = useCallback(() => {
    setError('')
    setUIState('init-form')
  }, [])

  const handleEnterClick = useCallback(() => {
    setError('')
    setUIState('enter-form')
  }, [])

  const handleBack = useCallback(() => {
    setError('')
    setUIState('setup')
  }, [])

  const handleCreateCircle = useCallback(async () => {
    setError('')
    setLoading(true)
    try {
      await saveIdentityIfNeeded()
      // If no user identity yet, create one first to get mnemonic
      const identity = await getIdentity()
      if (!identity.has_user_key && userName.trim()) {
        const result = await createUserIdentity(userName.trim())
        setMnemonic(result.mnemonic)
        // Proceed to init circle after showing backup screen
        await initCircle(circleName.trim() || 'DEFAULT', userName.trim() || undefined, joinPolicy)
        setUIState('mnemonic-backup')
        return
      }
      await initCircle(circleName.trim() || 'DEFAULT', userName.trim() || undefined, joinPolicy)
      triggerEruptionAndComplete()
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [circleName, joinPolicy, userName, deviceLabel])

  const handleJoinCircle = useCallback(async () => {
    setError('')
    setLoading(true)
    try {
      await saveIdentityIfNeeded()
      await enterCircle(inviteUri.trim(), userName.trim() || undefined)
      triggerEruptionAndComplete()
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [inviteUri, userName, deviceLabel])

  const handleLinkDevice = useCallback(async () => {
    setError('')
    setLoading(true)
    try {
      const result = await linkDevice(linkHandle.trim(), linkMnemonic.trim())
      setUserName(result.user_handle)
      setLinkSuccess(true)
      setLinkExpanded(false)
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [linkHandle, linkMnemonic])

  const handleMnemonicConfirmed = useCallback(() => {
    triggerEruptionAndComplete()
  }, [])

  // ── Shared input style ────────────────────────────────────────────────────

  const inputStyle: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 11,
    fontWeight: 700,
    background: '#fff',
    color: '#000',
    border: '2px solid #000',
    outline: 'none',
    padding: '5px 8px',
    width: '100%',
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    minHeight: 28,
  }

  const labelStyle: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 10,
    fontWeight: 800,
    color: '#555',
    textTransform: 'uppercase',
    letterSpacing: '0.1em',
    display: 'block',
    marginBottom: 3,
  }

  const rowStyle: React.CSSProperties = {
    display: 'grid',
    gridTemplateColumns: '90px 1fr',
    gap: 8,
    alignItems: 'center',
    marginBottom: 8,
  }

  const btnPrimary: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 11,
    fontWeight: 800,
    textTransform: 'uppercase',
    letterSpacing: '0.08em',
    background: '#000',
    color: '#fff',
    border: '2px solid #000',
    padding: '8px 12px',
    cursor: 'pointer',
    width: '100%',
    minHeight: 34,
  }

  const btnSecondary: React.CSSProperties = {
    ...btnPrimary,
    background: '#fff',
    color: '#000',
  }

  const btnGhost: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 10,
    fontWeight: 700,
    textTransform: 'uppercase',
    letterSpacing: '0.08em',
    background: 'transparent',
    color: '#555',
    border: 'none',
    padding: '4px 0',
    cursor: 'pointer',
    textAlign: 'left',
  }

  const dividerStyle: React.CSSProperties = {
    borderBottom: '1px solid #000',
    margin: '10px 0',
  }

  // ── Render panels ─────────────────────────────────────────────────────────

  function renderSetup() {
    return (
      <>
        {/* Header */}
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px', display: 'flex', alignItems: 'center', gap: 10 }}>
          <div style={{
            width: 22, height: 22,
            border: '2px solid #000',
            background: '#000',
            color: '#fff',
            fontFamily: 'var(--font-mono)',
            fontWeight: 800,
            fontSize: 13,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            flexShrink: 0,
          }}>E</div>
          <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 800, fontSize: 12, letterSpacing: '0.12em' }}>
            ENOXIAN PROTOCOL
          </div>
        </div>

        <div style={{ padding: '10px 12px 0' }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', fontWeight: 700, letterSpacing: '0.12em', marginBottom: 12 }}>
            PEER WORKSPACE &middot; HUMAN + AGENT
          </div>

          {/* ── Identity section ──────────────────────────────────────────── */}
          {!isEditingIdentity && userName ? (
            /* Compact identity line when already configured */
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 14, borderBottom: '1px dashed #ccc', paddingBottom: 10 }}>
              <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 800, letterSpacing: '0.06em' }}>
                {userName}
                <span style={{ fontWeight: 400, color: '#555', marginLeft: 6 }}>&middot; {deviceLabel}</span>
              </div>
              <button style={btnGhost} onClick={() => setIsEditingIdentity(true)}>EDIT</button>
            </div>
          ) : (
            /* Input fields for first run or when editing */
            <>
              <div style={rowStyle}>
                <span style={labelStyle}>YOUR NAME</span>
                <input
                  style={inputStyle}
                  type="text"
                  value={userName}
                  placeholder="HANDLE"
                  autoFocus={!userName}
                  onChange={e => setUserName(e.target.value)}
                />
              </div>
              <div style={{ ...rowStyle, marginBottom: 10 }}>
                <span style={labelStyle}>DEVICE</span>
                <input
                  style={inputStyle}
                  type="text"
                  value={deviceLabel}
                  placeholder="AUTO-DETECTED"
                  onChange={e => setDeviceLabel(e.target.value)}
                />
              </div>
              {isEditingIdentity && userName && (
                <div style={{ marginBottom: 10, textAlign: 'right' }}>
                  <button style={btnGhost} onClick={async () => {
                    await saveIdentityIfNeeded()
                    setIsEditingIdentity(false)
                  }}>SAVE</button>
                </div>
              )}
              <div style={{ ...dividerStyle, marginBottom: 12 }} />
            </>
          )}

          {error && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#000', background: '#fff', border: '2px solid #000', padding: '4px 8px', marginBottom: 8, fontWeight: 700 }}>
              {error}
            </div>
          )}

          {linkSuccess && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', marginBottom: 8, fontWeight: 700 }}>
              DEVICE LINKED &middot; IDENTITY LOADED
            </div>
          )}

          {/* Primary actions */}
          <div style={{ display: 'grid', gap: 6, marginBottom: 8 }}>
            <button style={btnPrimary} onClick={handleInitClick} disabled={loading}>
              INIT NEW CIRCLE
            </button>
            <button style={btnSecondary} onClick={handleEnterClick} disabled={loading}>
              ENTER VIA INVITE
            </button>
          </div>
        </div>

        {/* Link another device — only shown when no cryptographic user key yet */}
        {!hasUserKey && (
          <div style={{ borderTop: '1px solid #ddd', padding: '8px 12px' }}>
            <button style={btnGhost} onClick={() => setLinkExpanded(v => !v)}>
              {linkExpanded ? '↑' : '↓'} LINK THIS DEVICE TO EXISTING USER
            </button>

            {linkExpanded && (
              <div style={{ marginTop: 8 }}>
                <div style={dividerStyle} />
                <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', fontWeight: 700, marginBottom: 8, letterSpacing: '0.06em' }}>
                  ENTER YOUR HANDLE + MNEMONIC FROM ANOTHER DEVICE
                </div>
                <div style={rowStyle}>
                  <span style={labelStyle}>HANDLE</span>
                  <input style={inputStyle} type="text" value={linkHandle} onChange={e => setLinkHandle(e.target.value)} />
                </div>
                <div style={{ ...rowStyle, alignItems: 'flex-start' }}>
                  <span style={{ ...labelStyle, paddingTop: 6 }}>MNEMONIC</span>
                  <textarea
                    style={{ ...inputStyle, minHeight: 60, resize: 'vertical', fontFamily: 'var(--font-mono)', fontSize: 10 } as React.CSSProperties}
                    value={linkMnemonic}
                    onChange={e => setLinkMnemonic(e.target.value)}
                    placeholder="24 WORDS"
                  />
                </div>
                <button style={{ ...btnSecondary, marginTop: 4 }} onClick={handleLinkDevice} disabled={loading}>
                  {loading ? '...' : 'LINK DEVICE'}
                </button>
              </div>
            )}
          </div>
        )}
      </>
    )
  }

  function renderInitForm() {
    return (
      <>
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px' }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 800, fontSize: 11, letterSpacing: '0.1em' }}>INIT NEW CIRCLE</div>
        </div>
        <div style={{ padding: '12px 12px 10px' }}>
          <div style={dividerStyle} />
          <div style={rowStyle}>
            <span style={labelStyle}>CIRCLE NAME</span>
            <input
              style={inputStyle}
              type="text"
              value={circleName}
              placeholder="NAME"
              onChange={e => setCircleName(e.target.value)}
              autoFocus
            />
          </div>
          <div style={{ ...rowStyle, marginBottom: 12 }}>
            <span style={labelStyle}>JOIN POLICY</span>
            <div style={{ display: 'flex', gap: 0 }}>
              <button
                style={{
                  ...btnSecondary,
                  width: 'auto',
                  flex: 1,
                  background: joinPolicy === 'auto' ? '#000' : '#fff',
                  color: joinPolicy === 'auto' ? '#fff' : '#000',
                }}
                onClick={() => setJoinPolicy('auto')}
              >AUTO</button>
              <button
                style={{
                  ...btnSecondary,
                  width: 'auto',
                  flex: 1,
                  marginLeft: -2,
                  background: joinPolicy === 'manual' ? '#000' : '#fff',
                  color: joinPolicy === 'manual' ? '#fff' : '#000',
                }}
                onClick={() => setJoinPolicy('manual')}
              >MANUAL</button>
            </div>
          </div>

          {error && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#000', background: '#fff', border: '2px solid #000', padding: '4px 8px', marginBottom: 8, fontWeight: 700 }}>
              {error}
            </div>
          )}

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 6 }}>
            <button style={btnPrimary} onClick={handleCreateCircle} disabled={loading}>
              {loading ? '...' : 'CREATE'}
            </button>
            <button style={btnSecondary} onClick={handleBack} disabled={loading}>BACK</button>
          </div>
        </div>
      </>
    )
  }

  function renderEnterForm() {
    return (
      <>
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px' }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 800, fontSize: 11, letterSpacing: '0.1em' }}>ENTER VIA INVITE</div>
        </div>
        <div style={{ padding: '12px 12px 10px' }}>
          <div style={dividerStyle} />
          <div style={{ ...rowStyle, alignItems: 'flex-start', marginBottom: 12 }}>
            <span style={{ ...labelStyle, paddingTop: 6 }}>INVITE URI</span>
            <textarea
              style={{
                ...inputStyle,
                minHeight: 64,
                resize: 'vertical',
                fontFamily: 'var(--font-mono)',
                fontSize: 10,
                textTransform: 'none',
              } as React.CSSProperties}
              value={inviteUri}
              onChange={e => setInviteUri(e.target.value)}
              placeholder="PASTE URI"
              autoFocus
            />
          </div>

          {error && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#000', background: '#fff', border: '2px solid #000', padding: '4px 8px', marginBottom: 8, fontWeight: 700 }}>
              {error}
            </div>
          )}

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 6 }}>
            <button style={btnPrimary} onClick={handleJoinCircle} disabled={loading}>
              {loading ? '...' : 'JOIN'}
            </button>
            <button style={btnSecondary} onClick={handleBack} disabled={loading}>BACK</button>
          </div>
        </div>
      </>
    )
  }

  function renderMnemonicBackup() {
    return (
      <>
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px' }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 800, fontSize: 11, letterSpacing: '0.1em' }}>BACKUP YOUR MNEMONIC</div>
        </div>
        <div style={{ padding: '12px 12px 10px' }}>
          <div style={dividerStyle} />
          <p style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', fontWeight: 700, marginBottom: 10, lineHeight: 1.5 }}>
            WRITE THESE WORDS AND KEEP THEM SAFE.<br />
            YOU NEED THEM TO LINK OTHER DEVICES.
          </p>
          <div style={{
            border: '2px solid #000',
            padding: '10px 12px',
            fontFamily: 'var(--font-mono)',
            fontSize: 11,
            fontWeight: 700,
            lineHeight: 1.8,
            marginBottom: 12,
            wordBreak: 'break-word',
            background: '#fff',
            color: '#000',
            letterSpacing: '0.03em',
          }}>
            {mnemonic || '(GENERATING...)'}
          </div>
          <button style={btnPrimary} onClick={handleMnemonicConfirmed}>
            I HAVE BACKED UP MY MNEMONIC
          </button>
        </div>
      </>
    )
  }

  // ── JSX ───────────────────────────────────────────────────────────────────

  return (
    <>
      {/* Three.js angel canvas — full screen, behind everything */}
      <div
        ref={mountRef}
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 1,
          mixBlendMode: 'multiply',
        }}
      />

      {/* UI overlay — hidden once eruption begins */}
      <div
        style={{
          position: 'fixed',
          bottom: '8vh',
          left: '50%',
          transform: 'translateX(-50%)',
          zIndex: 10,
          width: 420,
          maxWidth: 'calc(100vw - 24px)',
          opacity: isErupting ? 0 : 1,
          pointerEvents: isErupting ? 'none' : 'auto',
          transition: 'opacity 0.4s ease',
        }}
      >
        <div
          className="sys-window"
          style={{
            background: '#fff',
            border: '2px solid #000',
            boxShadow: '6px 6px 0 #000',
            position: 'relative',
          }}
        >
          {uiState === 'setup' && renderSetup()}
          {uiState === 'init-form' && renderInitForm()}
          {uiState === 'enter-form' && renderEnterForm()}
          {uiState === 'mnemonic-backup' && renderMnemonicBackup()}
        </div>
      </div>
    </>
  )
}
