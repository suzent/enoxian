import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import {
  createDitheredComposer,
  addDitherLights,
  makeDitherMaterials,
  EXPOSURE_FADE_START,
  EXPOSURE_VOID,
  easeInOut,
} from '../lib/ditherShader'
import { makeCircleGeometry, makeShapeParams } from '../lib/circleShape'

interface Props { circleName: string }

export default function VoidOverlay({ circleName }: Props) {
  const mountRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const mount = mountRef.current
    if (!mount) return

    const W = window.innerWidth, H = window.innerHeight

    const renderer = new THREE.WebGLRenderer({ antialias: false })
    renderer.setSize(W, H)
    renderer.setPixelRatio(1)
    renderer.setClearColor(0xffffff, 1)
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    scene.background = new THREE.Color(0xffffff)
    const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 100)
    camera.position.z = 9

    addDitherLights(scene)
    const { flat, smooth } = makeDitherMaterials()

    // Circle's Platonic solid — flat-shaded for sharp per-face dither bands
    const baseGeo = makeCircleGeometry(circleName)
    const shape = new THREE.Mesh(baseGeo, flat)
    const params = makeShapeParams(circleName)
    shape.rotation.x = params.initRotX
    shape.rotation.y = params.initRotY
    shape.scale.setScalar(1.3)
    scene.add(shape)

    // ∅ ring — smooth-shaded for gradient dither bands
    const ring = new THREE.Mesh(new THREE.TorusGeometry(3.0, 0.1, 8, 128), smooth)
    ring.rotation.x = 0.15
    ring.rotation.z = 0.05
    scene.add(ring)

    // ∅ slash
    const slash = new THREE.Mesh(new THREE.CylinderGeometry(0.04, 0.04, 7.0, 8), smooth.clone())
    slash.rotation.z = Math.PI / 4
    scene.add(slash)

    const dc = createDitheredComposer(renderer, scene, camera, W, H)
    dc.setExposure(EXPOSURE_FADE_START)

    let raf = 0
    const startTime = performance.now()

    const tick = () => {
      raf = requestAnimationFrame(tick)
      const t = Math.min((performance.now() - startTime) / 1000, 1)
      dc.setExposure(EXPOSURE_FADE_START - (EXPOSURE_FADE_START - EXPOSURE_VOID) * easeInOut(t))
      shape.rotation.x += params.rotX * 0.22
      shape.rotation.y += params.rotY * 0.22
      ring.rotation.z += 0.0007
      ring.scale.setScalar(1 + Math.sin(performance.now() * 0.00035) * 0.025)
      dc.composer.render()
    }
    tick()

    const onResize = () => {
      const nW = window.innerWidth, nH = window.innerHeight
      dc.setSize(nW, nH)
      camera.aspect = nW / nH
      camera.updateProjectionMatrix()
    }
    window.addEventListener('resize', onResize)

    return () => {
      cancelAnimationFrame(raf)
      window.removeEventListener('resize', onResize)
      if (mount.contains(renderer.domElement)) mount.removeChild(renderer.domElement)
      baseGeo.dispose()
      renderer.dispose()
    }
  }, [circleName])

  return (
    <div
      ref={mountRef}
      style={{ position: 'fixed', inset: 0, zIndex: 60, pointerEvents: 'none', mixBlendMode: 'multiply' }}
    />
  )
}
