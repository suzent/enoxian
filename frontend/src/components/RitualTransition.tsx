import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import { createDitheredComposer } from '../lib/ditherShader'
import { makeCircleGeometry, applyCircleRotation } from '../lib/circleShape'

export type RitualMode = 'init' | 'enter'

interface Props {
  ritual: { mode: RitualMode; label?: string } | null
  onComplete: () => void
}

export default function RitualTransition({ ritual, onComplete }: Props) {
  const mountRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!ritual || !mountRef.current) return

    let raf = 0
    let closing = false
    const started = performance.now()
    const mount = mountRef.current
    
    const W = window.innerWidth
    const H = window.innerHeight
    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false })
    renderer.setPixelRatio(window.devicePixelRatio)
    renderer.setSize(W, H)
    renderer.setClearColor(0xffffff, 1)
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    scene.background = new THREE.Color(0xf0f0f0)

    const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 150)
    const baseCamPos = new THREE.Vector3(0, 0, 16)
    camera.position.copy(baseCamPos)
    camera.lookAt(0, 0, 0)

    // Lighting
    scene.add(new THREE.AmbientLight(0xffffff, 0.2))
    const keyLight = new THREE.DirectionalLight(0xffffff, 4.0)
    keyLight.position.set(-15, 20, 15)
    scene.add(keyLight)
    
    const rimLight = new THREE.DirectionalLight(0xffffff, 2.0)
    rimLight.position.set(15, -5, -15)
    scene.add(rimLight)
    
    const fillLight = new THREE.DirectionalLight(0xffffff, 0.4)
    fillLight.position.set(0, 5, 5)
    scene.add(fillLight)

    renderer.outputColorSpace = THREE.LinearSRGBColorSpace

    const dc = createDitheredComposer(renderer, scene, camera, W, H)

    const root = new THREE.Group()
    scene.add(root)

    const atmosGroup = new THREE.Group()
    root.add(atmosGroup)

    const matSolid = new THREE.MeshBasicMaterial({ color: 0x111111 }) 
    const matAccent = new THREE.MeshBasicMaterial({ color: 0x444444 })

    const createLine = (x: number, y: number, z: number, w: number, h: number, d: number, mat: THREE.Material = matSolid) => {
      const mesh = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), mat)
      mesh.position.set(x, y, z)
      return mesh
    }

    // Scattered Floating Crosses (+)
    const crossGroup = new THREE.Group()
    for (let i = 0; i < 20; i++) {
      const x = (Math.random() - 0.5) * 40
      const y = (Math.random() - 0.5) * 30
      const z = (Math.random() - 0.5) * 20 - 10
      const size = 0.2 + Math.random() * 0.3
      const cross = new THREE.Group()
      cross.add(createLine(0, 0, 0, size, 0.04, 0.04, matAccent))
      cross.add(createLine(0, 0, 0, 0.04, size, 0.04, matAccent))
      cross.position.set(x, y, z)
      crossGroup.add(cross)
    }
    atmosGroup.add(crossGroup)

    // Stardust / Particle Field
    const particleCount = 1500
    const pGeo = new THREE.BufferGeometry()
    const pPos = new Float32Array(particleCount * 3)
    for (let i = 0; i < particleCount; i++) {
      const radius = 5 + Math.random() * 25
      const theta = Math.random() * Math.PI * 2
      const x = Math.sin(theta) * radius
      const y = (Math.random() - 0.5) * 30
      const z = (Math.random() - 0.5) * 15 - 5
      pPos[i*3] = x
      pPos[i*3+1] = y
      pPos[i*3+2] = z
    }
    pGeo.setAttribute('position', new THREE.BufferAttribute(pPos, 3))
    const pMat = new THREE.PointsMaterial({ size: 0.1, color: 0x888888 })
    const particles = new THREE.Points(pGeo, pMat)
    atmosGroup.add(particles)

    // Halos
    const ringMat = new THREE.MeshPhongMaterial({ 
      color: 0x222222, 
      emissive: 0x111111,
      specular: 0xaaaaaa,
      shininess: 30,
      transparent: true,
      opacity: 0.6,
      depthWrite: false,
      side: THREE.DoubleSide
    })
    const halos = new THREE.Group()
    const halo1 = new THREE.Mesh(new THREE.TorusGeometry(6, 0.15, 4, 64), ringMat)
    halos.add(halo1)
    const halo2 = new THREE.Mesh(new THREE.TorusGeometry(8, 0.25, 4, 64), ringMat)
    halo2.rotation.x = Math.PI / 2
    halos.add(halo2)
    root.add(halos)

    // The generated circle shape
    const circleName = ritual.label || 'unknown_ritual'
    const circleGroup = makeCircleGeometry(circleName)
    circleGroup.scale.setScalar(2.5) // Scale up to be the centerpiece
    root.add(circleGroup)

    // Ethereal circular paths
    const pathMaterial = new THREE.LineBasicMaterial({ color: 0x111111, transparent: true, opacity: 0.42 })
    const paths = new THREE.Group()
    for (let i = 0; i < 8; i++) {
      const points: THREE.Vector3[] = []
      const radius = 4.5 + i * 0.3
      for (let j = 0; j <= 120; j++) {
        const a = (j / 120) * Math.PI * 2
        points.push(new THREE.Vector3(
          Math.cos(a) * radius,
          Math.sin(a * 3 + i) * 0.2 + 0.5,
          Math.sin(a) * radius,
        ))
      }
      const line = new THREE.Line(new THREE.BufferGeometry().setFromPoints(points), pathMaterial)
      line.rotation.x = Math.PI / 2 + i * 0.05
      line.rotation.z = i * 0.2
      paths.add(line)
    }
    root.add(paths)

    const resize = () => {
      const nW = window.innerWidth
      const nH = window.innerHeight
      camera.aspect = nW / nH
      camera.updateProjectionMatrix()
      renderer.setSize(nW, nH)
      dc.setSize(nW, nH)
    }
    window.addEventListener('resize', resize)

    const completeTimer = window.setTimeout(() => {
      closing = true
      mount.classList.add('ritual-closing')
    }, 4200)
    const doneTimer = window.setTimeout(onComplete, 5000)

    const lerp = (a: number, b: number, t: number) => a + (b - a) * t
    const easeOutExpo = (t: number) => t >= 1 ? 1 : 1 - Math.pow(2, -10 * t)
    const clamp01 = (v: number) => Math.max(0, Math.min(1, v))

    const render = (now: number) => {
      const t = (now - started) / 1000
      
      const et = clamp01((now - started) / 1500)
      const closeEase = closing ? clamp01((now - started - 4200) / 800) : 0

      // Animate Geometry
      halo1.rotation.z -= 0.008
      halo1.rotation.x += 0.004
      halo2.rotation.y += 0.012
      halo2.rotation.z += 0.006

      particles.rotation.y = t * 0.05
      crossGroup.rotation.y = t * 0.03
      
      paths.rotation.z = t * 0.15
      paths.rotation.y = t * 0.1
      paths.rotation.x = Math.sin(t * 0.4) * 0.1

      applyCircleRotation(circleGroup, circleName, now)

      // Dramatic scene rotation
      root.rotation.y = t * 0.2
      root.rotation.x = Math.sin(t * 0.5) * 0.1
      
      // When closing, shrink the root and move the camera to simulate moving into the dock
      root.scale.setScalar(1 - closeEase * 0.6)

      // Get target dock bounds if closing
      if (closing) {
        const dockEl = document.querySelector('[data-circle-dock]')
        if (dockEl) {
          const rect = dockEl.getBoundingClientRect()
          const dockCenterX = rect.left + rect.width / 2
          const dockCenterY = rect.top + rect.height / 2
          
          // Map screen coordinates to NDC (-1 to 1)
          const ndcX = (dockCenterX / window.innerWidth) * 2 - 1
          const ndcY = -(dockCenterY / window.innerHeight) * 2 + 1
          
          // Shift camera to simulate object moving towards target
          // This is a rough approximation without full projection unproject mapping
          camera.position.x = lerp(0, ndcX * -8, easeOutExpo(closeEase))
          camera.position.y = lerp(0, ndcY * -8, easeOutExpo(closeEase))
        }
      }

      // Start zoomed in, ease out to full view, zoom in slightly on close
      camera.position.z = lerp(4, 16, easeOutExpo(et)) - closeEase * 6

      // Keep constant exposure
      dc.setExposure(1.8)

      // Use CSS opacity to dissolve to the app page smoothly when closing
      if (closing) {
         mount.style.opacity = (1 - closeEase).toString()
      }

      dc.composer.render()
      raf = requestAnimationFrame(render)
    }

    raf = requestAnimationFrame(render)

    return () => {
      cancelAnimationFrame(raf)
      window.clearTimeout(completeTimer)
      window.clearTimeout(doneTimer)
      window.removeEventListener('resize', resize)
      mount.classList.remove('ritual-closing')
      if (renderer.domElement.parentNode === mount) {
        mount.removeChild(renderer.domElement)
      }
      renderer.forceContextLoss()
      renderer.dispose()
      dc.composer.dispose()
      
      root.traverse((child) => {
        const m = child as THREE.Mesh
        if (m.geometry) m.geometry.dispose()
        if (m.material) {
          if (Array.isArray(m.material)) {
            m.material.forEach(mat => mat.dispose())
          } else {
            m.material.dispose()
          }
        }
      })
    }
  }, [ritual, onComplete])

  if (!ritual) return null

  return (
    <div className="ritual-overlay" aria-live="polite">
      <div ref={mountRef} className="ritual-canvas" />
      <div className="ritual-caption" style={{ zIndex: 10 }}>
        <span>{ritual.mode === 'init' ? 'casting circle' : 'entering circle'}</span>
        {ritual.label && <strong>{ritual.label}</strong>}
      </div>
    </div>
  )
}
