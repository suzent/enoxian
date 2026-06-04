import { useEffect, useRef, useImperativeHandle, forwardRef } from 'react'
import * as THREE from 'three'
import { useApp } from '../context/AppContext'

export interface SceneHandle {
  pulse(x?: number, z?: number): void
  setPeers(peers: { id: string; online: boolean }[]): void
}

const GRID = 100
const SPREAD = 28

const VERT = `
  uniform float uTime;
  uniform vec2 uRipple;
  uniform float uRippleAge;
  uniform float uManifestLerp;
  
  attribute float aBase;
  attribute vec3 aTargetPos;
  
  varying float vOpacity;
  varying float vManifest;

  void main() {
    vec3 pos = position;

    // Standing wave deformation for the void grid
    float wave = sin(pos.x * 0.35 + uTime * 0.6) * cos(pos.z * 0.35 + uTime * 0.4) * 0.55;
    wave += sin(pos.x * 0.7 + uTime * 0.3) * sin(pos.z * 0.5 + uTime * 0.5) * 0.2;
    pos.y = wave;

    // Morph towards the occult Torus Knot
    vec3 finalPos = mix(pos, aTargetPos, uManifestLerp);

    // Ripple from event origin
    float dist = length(finalPos.xz - uRipple);
    float rippleStrength = exp(-uRippleAge * 0.9) * exp(-dist * 0.18);
    float ripple = sin(dist * 2.2 - uRippleAge * 6.0) * rippleStrength * 1.2;
    finalPos.y += ripple;

    // Boost opacity when manifesting
    vOpacity = aBase + rippleStrength * 0.5 + (uManifestLerp * 0.5);
    vManifest = uManifestLerp;

    // Decrease point size and tighten the range for a finer aesthetic
    float pSize = mix(1.0, 1.4, uManifestLerp);
    gl_PointSize = pSize * (40.0 / -(modelViewMatrix * vec4(finalPos, 1.0)).z);
    gl_Position = projectionMatrix * modelViewMatrix * vec4(finalPos, 1.0);
  }
`

const FRAG = `
  varying float vOpacity;
  varying float vManifest;
  
  void main() {
    float d = length(gl_PointCoord - 0.5);
    // Sharp, hard-edged pixels for brutalist/hacker aesthetic
    if (d > 0.5) discard;
    
    // Void color: light slate/grey
    vec3 voidColor = vec3(0.6, 0.6, 0.6);
    
    // Manifest color: absolute void black / obsidian
    vec3 manifestColor = vec3(0.067, 0.067, 0.067);
    
    vec3 finalColor = mix(voidColor, manifestColor, vManifest);
    // High contrast opacity boost for manifest
    float finalOpacity = mix(vOpacity, vOpacity * 1.5, vManifest);
    gl_FragColor = vec4(finalColor, clamp(finalOpacity, 0.0, 1.0));
  }
`

const ThreeBackground = forwardRef<SceneHandle>((_, ref) => {
  const mountRef = useRef<HTMLDivElement>(null)
  
  // Connect to global app state to detect MANIFEST/VOID
  const { circles, activeCircleId } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)
  const isManifest = activeCircle ? !activeCircle.disabled : false
  const isManifestRef = useRef(isManifest)

  useEffect(() => {
    isManifestRef.current = isManifest
  }, [isManifest])

  const stateRef = useRef<{
    rippleCenter: THREE.Vector2
    rippleAge: number
    peerMeshes: THREE.Points[]
    peerLines: THREE.LineSegments | null
    peers: { id: string; online: boolean }[]
  }>({
    rippleCenter: new THREE.Vector2(0, 0),
    rippleAge: 999,
    peerMeshes: [],
    peerLines: null,
    peers: [],
  })

  useImperativeHandle(ref, () => ({
    pulse(x = 0, z = 0) {
      stateRef.current.rippleCenter.set(x, z)
      stateRef.current.rippleAge = 0
    },
    setPeers(peers) {
      stateRef.current.peers = peers
    },
  }))

  useEffect(() => {
    const el = mountRef.current
    if (!el) return

    // ── Scene setup ──────────────────────────────────────────────────────────
    const scene = new THREE.Scene()
    scene.fog = new THREE.FogExp2(0xeaeae4, 0.018)

    const camera = new THREE.PerspectiveCamera(42, el.clientWidth / el.clientHeight, 0.1, 200)
    camera.position.set(0, 14, 22)
    camera.lookAt(0, 0, 0)

    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true })
    renderer.setPixelRatio(Math.min(devicePixelRatio, 2))
    renderer.setSize(el.clientWidth, el.clientHeight)
    renderer.setClearColor(0xeaeae4, 1)
    el.appendChild(renderer.domElement)

    // ── Lattice grid & Target Geometry ───────────────────────────────────────
    const count = GRID * GRID
    const positions = new Float32Array(count * 3)
    const targetPositions = new Float32Array(count * 3)
    const baseOpacity = new Float32Array(count)

    for (let i = 0; i < GRID; i++) {
      for (let j = 0; j < GRID; j++) {
        const idx = (i * GRID + j) * 3
        
        // 1. Base Grid (Void State) - Static terminal lattice
        const x = (i / (GRID - 1) - 0.5) * SPREAD + (Math.random() - 0.5) * 0.25
        const z = (j / (GRID - 1) - 0.5) * SPREAD + (Math.random() - 0.5) * 0.25
        positions[idx] = x
        positions[idx + 1] = 0
        positions[idx + 2] = z
        
        // Fade opacity toward edges for a vignette feel
        const edgeDist = Math.max(Math.abs(x) / (SPREAD / 2), Math.abs(z) / (SPREAD / 2))
        baseOpacity[i * GRID + j] = (1 - Math.pow(edgeDist, 2)) * 0.25 + Math.random() * 0.08

        // 2. Sacred Geometry / Ophanim (Manifest State)
        const pointIdx = i * GRID + j
        let tx = 0, ty = 0, tz = 0
        
        if (pointIdx < 3000) {
          // Outer Fibonacci Sphere (The Monolith Shell)
          const fIdx = pointIdx
          const phi = Math.acos(1 - 2 * (fIdx + 0.5) / 3000)
          const theta = Math.PI * (1 + Math.sqrt(5)) * fIdx
          const R = 15
          tx = R * Math.sin(phi) * Math.cos(theta)
          ty = R * Math.sin(phi) * Math.sin(theta)
          tz = R * Math.cos(phi)
        } else if (pointIdx < 5000) {
          // Inner Golden Core
          const fIdx = pointIdx - 3000
          const phi = Math.acos(1 - 2 * (fIdx + 0.5) / 2000)
          const theta = Math.PI * (1 + Math.sqrt(5)) * fIdx
          const R = 5
          tx = R * Math.sin(phi) * Math.cos(theta)
          ty = R * Math.sin(phi) * Math.sin(theta)
          tz = R * Math.cos(phi)
        } else {
          // The Intertwining Sigil (Torus Knot p=3, q=7)
          const kIdx = pointIdx - 5000
          const total = 5000
          const strand = kIdx % 4 // 4 intertwined strands
          const t = (kIdx / total) * Math.PI * 2
          const p = 3, q = 7
          const R = 9, r = 2.5
          
          const baseTx = (R + r * Math.cos(q * t)) * Math.cos(p * t)
          const baseTy = (R + r * Math.cos(q * t)) * Math.sin(p * t)
          const baseTz = r * Math.sin(q * t)
          
          // Offset strands mathematically to form a thick, complex wireframe
          tx = baseTx + Math.sin(t * 100 + strand * Math.PI / 2) * 0.5
          ty = baseTy + Math.cos(t * 100 + strand * Math.PI / 2) * 0.5
          tz = baseTz + Math.sin(t * 60 - strand * Math.PI / 2) * 0.5
        }
        
        targetPositions[idx] = tx
        targetPositions[idx + 1] = ty
        targetPositions[idx + 2] = tz
      }
    }

    const geo = new THREE.BufferGeometry()
    geo.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    geo.setAttribute('aTargetPos', new THREE.BufferAttribute(targetPositions, 3))
    geo.setAttribute('aBase', new THREE.BufferAttribute(baseOpacity, 1))

    const uTime = { value: 0 }
    const uRipple = { value: new THREE.Vector2(0, 0) }
    const uRippleAge = { value: 999.0 }
    const uManifestLerp = { value: 0.0 }

    const mat = new THREE.ShaderMaterial({
      uniforms: { uTime, uRipple, uRippleAge, uManifestLerp },
      vertexShader: VERT,
      fragmentShader: FRAG,
      transparent: true,
      depthWrite: false,
      blending: THREE.NormalBlending,
    })

    const lattice = new THREE.Points(geo, mat)
    scene.add(lattice)

    // ── Mouse parallax ───────────────────────────────────────────────────────
    const mouse = new THREE.Vector2(0, 0)
    const onMouseMove = (e: MouseEvent) => {
      mouse.set(
        (e.clientX / window.innerWidth - 0.5) * 2,
        -(e.clientY / window.innerHeight - 0.5) * 2,
      )
    }
    window.addEventListener('mousemove', onMouseMove)

    // ── Resize ───────────────────────────────────────────────────────────────
    const onResize = () => {
      camera.aspect = el.clientWidth / el.clientHeight
      camera.updateProjectionMatrix()
      renderer.setSize(el.clientWidth, el.clientHeight)
    }
    window.addEventListener('resize', onResize)

    // ── Animation loop ───────────────────────────────────────────────────────
    let frame = 0
    let raf: number
    const clock = new THREE.Clock()

    const animate = () => {
      raf = requestAnimationFrame(animate)
      const dt = clock.getDelta()
      frame += dt

      uTime.value = frame
      uRipple.value.copy(stateRef.current.rippleCenter)
      stateRef.current.rippleAge += dt * 1.2
      uRippleAge.value = stateRef.current.rippleAge

      // Smoothly ease towards the current state
      const targetLerp = isManifestRef.current ? 1.0 : 0.0
      uManifestLerp.value += (targetLerp - uManifestLerp.value) * 0.04

      // Interpolate Rotation: 
      // VOID state: subtle grid sway
      // MANIFEST state: continuously spinning 3D knot
      const voidRotY = Math.sin(frame * 0.04) * 0.08
      const manifestRotY = frame * 0.3
      const manifestRotX = frame * 0.2
      const manifestRotZ = frame * 0.1

      lattice.rotation.y = voidRotY + (manifestRotY - voidRotY) * uManifestLerp.value
      lattice.rotation.x = manifestRotX * uManifestLerp.value
      lattice.rotation.z = manifestRotZ * uManifestLerp.value

      // Dynamic Camera adjustment during MANIFEST
      camera.position.x += (mouse.x * 3 - camera.position.x) * 0.03
      
      // Pull the camera in slightly during manifest to emphasize the Torus
      const targetCamY = 14 + mouse.y * 2 - (4 * uManifestLerp.value)
      const targetCamZ = 22 - (2 * uManifestLerp.value)
      
      camera.position.y += (targetCamY - camera.position.y) * 0.03
      camera.position.z += (targetCamZ - camera.position.z) * 0.03
      
      camera.lookAt(0, 0, 0)

      renderer.render(scene, camera)
    }
    animate()

    return () => {
      cancelAnimationFrame(raf)
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('resize', onResize)
      if (el.contains(renderer.domElement)) {
        el.removeChild(renderer.domElement)
      }
      renderer.forceContextLoss()
      renderer.dispose()
    }
  }, [])

  return <div ref={mountRef} className="fixed inset-0 z-0" />
})

ThreeBackground.displayName = 'ThreeBackground'
export default ThreeBackground
