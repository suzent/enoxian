import { useEffect, useRef, useImperativeHandle, forwardRef } from 'react'
import * as THREE from 'three'

export interface SceneHandle {
  pulse(x?: number, z?: number): void
  setPeers(peers: { id: string; online: boolean }[]): void
}

const GRID = 60
const SPREAD = 28

const VERT = `
  uniform float uTime;
  uniform vec2 uRipple;
  uniform float uRippleAge;
  attribute float aBase;
  varying float vOpacity;

  void main() {
    vec3 pos = position;

    // Standing wave deformation
    float wave = sin(pos.x * 0.35 + uTime * 0.6) * cos(pos.z * 0.35 + uTime * 0.4) * 0.55;
    wave += sin(pos.x * 0.7 + uTime * 0.3) * sin(pos.z * 0.5 + uTime * 0.5) * 0.2;
    pos.y = wave;

    // Ripple from event origin
    float dist = length(pos.xz - uRipple);
    float rippleStrength = exp(-uRippleAge * 0.9) * exp(-dist * 0.18);
    float ripple = sin(dist * 2.2 - uRippleAge * 6.0) * rippleStrength * 1.2;
    pos.y += ripple;

    vOpacity = aBase + rippleStrength * 0.5;

    gl_PointSize = 1.6 * (40.0 / -(modelViewMatrix * vec4(pos, 1.0)).z);
    gl_Position = projectionMatrix * modelViewMatrix * vec4(pos, 1.0);
  }
`

const FRAG = `
  varying float vOpacity;
  void main() {
    float d = length(gl_PointCoord - 0.5);
    if (d > 0.5) discard;
    gl_FragColor = vec4(0.067, 0.067, 0.067, clamp(vOpacity, 0.0, 1.0));
  }
`

const ThreeBackground = forwardRef<SceneHandle>((_, ref) => {
  const mountRef = useRef<HTMLDivElement>(null)
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

    // ── Lattice grid ─────────────────────────────────────────────────────────
    const count = GRID * GRID
    const positions = new Float32Array(count * 3)
    const baseOpacity = new Float32Array(count)

    for (let i = 0; i < GRID; i++) {
      for (let j = 0; j < GRID; j++) {
        const idx = (i * GRID + j) * 3
        const x = (i / (GRID - 1) - 0.5) * SPREAD + (Math.random() - 0.5) * 0.25
        const z = (j / (GRID - 1) - 0.5) * SPREAD + (Math.random() - 0.5) * 0.25
        positions[idx] = x
        positions[idx + 1] = 0
        positions[idx + 2] = z
        // Fade opacity toward edges for a vignette feel
        const edgeDist = Math.max(Math.abs(x) / (SPREAD / 2), Math.abs(z) / (SPREAD / 2))
        baseOpacity[i * GRID + j] = (1 - Math.pow(edgeDist, 2)) * 0.25 + Math.random() * 0.08
      }
    }

    const geo = new THREE.BufferGeometry()
    geo.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    geo.setAttribute('aBase', new THREE.BufferAttribute(baseOpacity, 1))

    const uTime = { value: 0 }
    const uRipple = { value: new THREE.Vector2(0, 0) }
    const uRippleAge = { value: 999.0 }

    const mat = new THREE.ShaderMaterial({
      uniforms: { uTime, uRipple, uRippleAge },
      vertexShader: VERT,
      fragmentShader: FRAG,
      transparent: true,
      depthWrite: false,
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

      // Slow parallax tilt
      camera.position.x += (mouse.x * 3 - camera.position.x) * 0.03
      camera.position.y += (14 + mouse.y * 2 - camera.position.y) * 0.03
      camera.lookAt(0, 0, 0)

      // Slow lattice drift
      lattice.rotation.y = Math.sin(frame * 0.04) * 0.08

      renderer.render(scene, camera)
    }
    animate()

    return () => {
      cancelAnimationFrame(raf)
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('resize', onResize)
      renderer.dispose()
      el.removeChild(renderer.domElement)
    }
  }, [])

  return <div ref={mountRef} className="fixed inset-0 z-0" />
})

ThreeBackground.displayName = 'ThreeBackground'
export default ThreeBackground
