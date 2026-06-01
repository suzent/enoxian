import { useEffect, useRef } from 'react'
import * as THREE from 'three'

export type RitualMode = 'init' | 'enter'

interface Props {
  ritual: { mode: RitualMode; label?: string } | null
  onComplete: () => void
}

function makeDitherTexture() {
  const size = 64
  const canvas = document.createElement('canvas')
  canvas.width = size
  canvas.height = size
  const ctx = canvas.getContext('2d')!
  ctx.fillStyle = '#f7f7f2'
  ctx.fillRect(0, 0, size, size)
  ctx.fillStyle = '#111111'
  for (let y = 0; y < size; y += 4) {
    for (let x = 0; x < size; x += 4) {
      const v = (x * 13 + y * 7) % 17
      if (v < 9) ctx.fillRect(x, y, 1.5, 1.5)
      if (v < 4) ctx.fillRect(x + 2, y + 2, 1, 1)
    }
  }
  const texture = new THREE.CanvasTexture(canvas)
  texture.wrapS = THREE.RepeatWrapping
  texture.wrapT = THREE.RepeatWrapping
  texture.repeat.set(10, 10)
  texture.magFilter = THREE.NearestFilter
  texture.minFilter = THREE.NearestFilter
  return texture
}

export default function RitualTransition({ ritual, onComplete }: Props) {
  const mountRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!ritual || !mountRef.current) return

    let frame = 0
    let closing = false
    let raf = 0
    const started = performance.now()
    const mount = mountRef.current
    const renderer = new THREE.WebGLRenderer({ antialias: false, alpha: true })
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
    renderer.setSize(window.innerWidth, window.innerHeight)
    renderer.setClearColor(0xf7f7f2, 1)
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    scene.fog = new THREE.Fog(0xf7f7f2, 12, 38)
    const camera = new THREE.PerspectiveCamera(42, window.innerWidth / window.innerHeight, 0.1, 100)
    camera.position.set(0, 3.2, 13.5)
    camera.lookAt(0, 0, 0)

    const root = new THREE.Group()
    scene.add(root)

    const dither = makeDitherTexture()
    const pale = new THREE.MeshBasicMaterial({
      color: 0xf7f7f2,
      map: dither,
      side: THREE.DoubleSide,
    })
    const black = new THREE.MeshBasicMaterial({ color: 0x111111, side: THREE.DoubleSide })
    const veil = new THREE.MeshBasicMaterial({
      color: 0x111111,
      transparent: true,
      opacity: 0.16,
      wireframe: true,
    })

    const torus = new THREE.Mesh(new THREE.TorusGeometry(3.7, 0.22, 8, 160), black)
    torus.rotation.x = Math.PI / 2.15
    root.add(torus)

    const torusGhost = new THREE.Mesh(new THREE.TorusGeometry(4.35, 0.035, 6, 192), veil)
    torusGhost.rotation.x = Math.PI / 2.08
    root.add(torusGhost)

    const monolith = new THREE.Mesh(new THREE.OctahedronGeometry(1.85, 0), pale)
    monolith.scale.set(1, 1.55, 0.72)
    monolith.position.y = 0.5
    root.add(monolith)

    const inner = new THREE.Mesh(new THREE.IcosahedronGeometry(1.15, 1), veil)
    inner.position.y = 0.5
    root.add(inner)

    const pathMaterial = new THREE.LineBasicMaterial({ color: 0x111111, transparent: true, opacity: 0.42 })
    const paths = new THREE.Group()
    for (let i = 0; i < 10; i++) {
      const points: THREE.Vector3[] = []
      const radius = 2.1 + i * 0.27
      for (let j = 0; j <= 180; j++) {
        const a = (j / 180) * Math.PI * 2
        points.push(new THREE.Vector3(
          Math.cos(a) * radius,
          Math.sin(a * 3 + i) * 0.16 + 0.45,
          Math.sin(a) * radius,
        ))
      }
      const line = new THREE.Line(new THREE.BufferGeometry().setFromPoints(points), pathMaterial)
      line.rotation.x = Math.PI / 2 + i * 0.045
      line.rotation.z = i * 0.18
      paths.add(line)
    }
    root.add(paths)

    const nodes = new THREE.Group()
    const nodeMaterial = new THREE.MeshBasicMaterial({ color: 0x111111 })
    const nodeGeometry = new THREE.BoxGeometry(0.12, 0.12, 0.12)
    for (let i = 0; i < 18; i++) {
      const angle = (i / 18) * Math.PI * 2
      const node = new THREE.Mesh(nodeGeometry, nodeMaterial)
      node.position.set(Math.cos(angle) * 5.1, Math.sin(i * 1.7) * 1.3 + 0.2, Math.sin(angle) * 5.1)
      nodes.add(node)
    }
    root.add(nodes)

    const resize = () => {
      camera.aspect = window.innerWidth / window.innerHeight
      camera.updateProjectionMatrix()
      renderer.setSize(window.innerWidth, window.innerHeight)
    }
    window.addEventListener('resize', resize)

    const completeTimer = window.setTimeout(() => {
      closing = true
      mount.classList.add('ritual-closing')
    }, 4200)
    const doneTimer = window.setTimeout(onComplete, 5000)

    const render = (now: number) => {
      const t = (now - started) / 1000
      frame += 1
      const closeEase = closing ? Math.min(1, (now - started - 4200) / 800) : 0
      root.rotation.y = t * 0.28
      root.rotation.x = Math.sin(t * 0.46) * 0.08
      torus.rotation.z = t * 0.34
      torusGhost.rotation.z = -t * 0.23
      monolith.rotation.y = t * 0.18
      monolith.rotation.z = Math.sin(t * 0.7) * 0.06
      inner.rotation.x = -t * 0.31
      inner.rotation.y = t * 0.21
      paths.rotation.z = t * 0.11
      nodes.rotation.y = -t * 0.16
      dither.offset.x = (frame % 240) / 2400
      dither.offset.y = (frame % 180) / 1800
      root.scale.setScalar(1 - closeEase * 0.45)
      camera.position.z = 13.5 - closeEase * 3.8 + Math.sin(t * 0.8) * 0.15
      renderer.render(scene, camera)
      raf = requestAnimationFrame(render)
    }

    raf = requestAnimationFrame(render)

    return () => {
      cancelAnimationFrame(raf)
      window.clearTimeout(completeTimer)
      window.clearTimeout(doneTimer)
      window.removeEventListener('resize', resize)
      mount.classList.remove('ritual-closing')
      renderer.dispose()
      dither.dispose()
      torus.geometry.dispose()
      torusGhost.geometry.dispose()
      monolith.geometry.dispose()
      inner.geometry.dispose()
      nodeGeometry.dispose()
      pale.dispose()
      black.dispose()
      veil.dispose()
      pathMaterial.dispose()
      nodeMaterial.dispose()
      mount.removeChild(renderer.domElement)
    }
  }, [ritual, onComplete])

  if (!ritual) return null

  return (
    <div className="ritual-overlay" aria-live="polite">
      <div ref={mountRef} className="ritual-canvas" />
      <div className="ritual-dither" />
      <div className="ritual-caption">
        <span>{ritual.mode === 'init' ? 'casting circle' : 'entering circle'}</span>
        {ritual.label && <strong>{ritual.label}</strong>}
      </div>
    </div>
  )
}
