import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import { applyCircleRotation, makeCircleGeometry } from '../lib/circleShape'
import {
  createDitheredComposer,
  makeDitherMaterials,
} from '../lib/ditherShader'
import {
  CIRCLE_EXPOSURE,
  createCircleCamera,
  createCircleRenderer,
  prepareCircleScene,
} from '../lib/circleRender'

interface Props {
  name: string
  size?: number
  className?: string
  title?: string
  voided?: boolean
}

export default function CircleGlyph({ name, size = 72, className, title, voided = false }: Props) {
  const mountRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    const mount = mountRef.current
    if (!mount) return

    const renderer = createCircleRenderer(size, size)
    renderer.domElement.style.cssText = `
      display:block;
      width:${size}px;height:${size}px;
      image-rendering:pixelated;
    `
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    prepareCircleScene(scene)

    const camera = createCircleCamera()

    const group = makeCircleGeometry(name)
    applyCircleRotation(group, name)
    group.scale.setScalar(1)
    scene.add(group)

    let ring: THREE.Mesh | null = null
    if (voided) {
      const { smooth } = makeDitherMaterials()
      ring = new THREE.Mesh(new THREE.TorusGeometry(1.28, 0.045, 8, 96), smooth)
      ring.rotation.x = 0.15
      ring.rotation.z = 0.05
      const slash = new THREE.Mesh(new THREE.CylinderGeometry(0.018, 0.018, 3.0, 8), smooth.clone())
      slash.rotation.z = Math.PI / 4
      scene.add(ring, slash)
    }

    const dc = createDitheredComposer(renderer, scene, camera, size, size)
    dc.setExposure(CIRCLE_EXPOSURE)

    let raf = 0
    const tick = () => {
      raf = requestAnimationFrame(tick)
      const now = performance.now()
      applyCircleRotation(group, name, now)
      if (ring) {
        ring.rotation.z += 0.002
        ring.scale.setScalar(1 + Math.sin(now * 0.00035) * 0.025)
      }
      dc.composer.render()
    }
    tick()

    return () => {
      cancelAnimationFrame(raf)
      dc.composer.dispose()
      renderer.forceContextLoss()
      renderer.dispose()
      renderer.domElement.remove()
    }
  }, [name, size, voided])

  return (
    <div
      ref={mountRef}
      className={className}
      title={title}
      aria-label={title}
      style={{
        width: size,
        height: size,
        display: 'block',
        mixBlendMode: 'multiply',
        flexShrink: 0,
      }}
    />
  )
}
