import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import { createDitheredComposer } from '../lib/ditherShader'
import { makeCircleGeometry, applyCircleRotation } from '../lib/circleShape'
import {
  CIRCLE_EXPOSURE,
  objectMaxDimension,
  prepareCircleScene,
  rectCenterOnCameraPlane,
  scaleForRectDimension,
} from '../lib/circleRender'
import { triggerDockBurst } from '../lib/particleEffect'

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
    const renderer = new THREE.WebGLRenderer({ antialias: false, alpha: false })
    renderer.setPixelRatio(1)
    renderer.setSize(W, H)
    renderer.setClearColor(0xffffff, 1)

    mount.style.cssText = 'position:fixed;inset:0;z-index:5000;mix-blend-mode:multiply;pointer-events:none;opacity:0;'
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    prepareCircleScene(scene)

    const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 150)
    const baseCamPos = new THREE.Vector3(0, 0, 16)
    camera.position.copy(baseCamPos)
    camera.lookAt(0, 0, 0)

    renderer.outputColorSpace = THREE.SRGBColorSpace

    const dc = createDitheredComposer(renderer, scene, camera, W, H)
    dc.setExposure(CIRCLE_EXPOSURE)

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
    const pMat = new THREE.PointsMaterial({ size: 0.1, color: 0x888888, transparent: true, opacity: 0 })
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
    const circleHome = new THREE.Vector3(0, 0, 0)
    const circleBaseScale = 2.5
    const circleMaxDimension = objectMaxDimension(circleGroup)
    circleGroup.scale.setScalar(1.45)
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
    const easeInOut = (t: number) => t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
    const clamp01 = (v: number) => Math.max(0, Math.min(1, v))
    const introMs = 900
    let burstFired = false

    const getDockTarget = () => {
      const targetEl = document.querySelector('[data-circle-dock] canvas') ?? document.querySelector('[data-circle-dock]')
      if (!targetEl) return null

      const rect = targetEl.getBoundingClientRect()
      const world = rectCenterOnCameraPlane(rect, camera)
      const scale = scaleForRectDimension(circleMaxDimension, rect, camera)
      return { world, scale }
    }

    const render = (now: number) => {
      const t = (now - started) / 1000
      
      const et = clamp01((now - started) / introMs)
      const introMotion = easeInOut(et)
      const closeEase = closing ? clamp01((now - started - 4200) / 800) : 0
      const closeMotion = easeInOut(closeEase)

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
      root.rotation.y = t * 0.2 * introMotion * (1 - closeMotion)
      root.rotation.x = Math.sin(t * 0.5) * 0.1 * introMotion * (1 - closeMotion)
      root.scale.setScalar(1)

      atmosGroup.scale.setScalar(1 + closeMotion * 0.14)
      atmosGroup.visible = closeEase < 0.98
      pMat.opacity = 0.72 * introMotion * (1 - closeMotion)
      ringMat.opacity = 0.6 * introMotion * (1 - closeMotion * 0.82)
      pathMaterial.opacity = 0.42 * introMotion * (1 - closeMotion)

      if (closing) {
        root.updateMatrixWorld(true)
        const dockTarget = getDockTarget()
        if (dockTarget) {
          const targetLocal = root.worldToLocal(dockTarget.world)
          circleGroup.position.lerpVectors(circleHome, targetLocal, closeMotion)
          circleGroup.scale.setScalar(lerp(circleBaseScale, dockTarget.scale, closeMotion))

          if (closeMotion >= 1 && !burstFired) {
            burstFired = true
            triggerDockBurst()
            // Pop-in the dock element as the ritual overlay dissolves
            const dockEl = document.querySelector('[data-circle-dock]') as HTMLElement | null
            if (dockEl) {
              dockEl.style.transition = 'none'
              dockEl.style.transform = 'scale(0.82)'
              dockEl.style.opacity = '0'
              requestAnimationFrame(() => {
                dockEl.style.transition = 'transform 300ms cubic-bezier(0.34,1.56,0.64,1), opacity 180ms ease-out'
                dockEl.style.transform = 'scale(1)'
                dockEl.style.opacity = '1'
                setTimeout(() => {
                  dockEl.style.transition = ''
                  dockEl.style.transform = ''
                  dockEl.style.opacity = ''
                }, 320)
              })
            }
          }
        }
      } else {
        circleGroup.position.copy(circleHome)
        circleGroup.scale.setScalar(lerp(1.45, circleBaseScale, introMotion))
      }

      camera.position.x = 0
      camera.position.y = 0
      camera.position.z = lerp(13, 16, easeOutExpo(et)) + closeMotion * 1.4
      camera.lookAt(0, 0, 0)

      dc.setExposure(CIRCLE_EXPOSURE)
      dc.composer.render()
      if (closing) {
        const dissolve = clamp01((closeEase - 0.72) / 0.28)
        mount.style.opacity = (1 - dissolve).toString()
      } else {
        mount.style.opacity = introMotion.toString()
      }
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
        <span>{ritual.mode === 'init' ? 'creating circle' : 'joining circle'}</span>
        {ritual.label && <strong>{ritual.label}</strong>}
      </div>
    </div>
  )
}
