import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import { applyCircleRotation, makeCircleGeometry } from '../lib/circleShape'
import {
  addDitherLights,
  createDitheredComposer,
  EXPOSURE_ICON,
  makeDitherMaterials,
} from '../lib/ditherShader'

interface Props {
  name: string
  size?: number
  className?: string
  title?: string
  voided?: boolean
}

export default function CircleGlyph({ name, size = 72, className, title, voided = false }: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const renderer = new THREE.WebGLRenderer({ antialias: false })
    renderer.setPixelRatio(1)
    renderer.setClearColor(0xffffff, 1)
    renderer.setSize(size, size)
    renderer.domElement.style.cssText = 'position:fixed;top:-9999px;left:-9999px;pointer-events:none;'
    document.body.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    scene.background = new THREE.Color(0xffffff)
    addDitherLights(scene)

    const camera = new THREE.PerspectiveCamera(50, 1, 0.1, 100)
    camera.position.z = 2.8

    const group = makeCircleGeometry(name)
    applyCircleRotation(group, name)
    group.scale.setScalar(voided ? 0.74 : 1)
    scene.add(group)

    let ring: THREE.Mesh | null = null
    let slash: THREE.Mesh | null = null
    if (voided) {
      const { smooth } = makeDitherMaterials()
      ring = new THREE.Mesh(new THREE.TorusGeometry(1.18, 0.045, 8, 96), smooth)
      ring.rotation.x = 0.15
      ring.rotation.z = 0.05
      slash = new THREE.Mesh(new THREE.CylinderGeometry(0.018, 0.018, 2.7, 8), smooth.clone())
      slash.rotation.z = Math.PI / 4
      scene.add(ring, slash)
    }

    const dc = createDitheredComposer(renderer, scene, camera, size, size)
    dc.setExposure(EXPOSURE_ICON)

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
      const ctx = canvas.getContext('2d')
      if (ctx) {
        ctx.clearRect(0, 0, canvas.width, canvas.height)
        ctx.drawImage(renderer.domElement, 0, 0, canvas.width, canvas.height)
      }
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
    <canvas
      ref={canvasRef}
      width={size}
      height={size}
      className={className}
      title={title}
      aria-label={title}
      style={{
        width: size,
        height: size,
        display: 'block',
        imageRendering: 'pixelated',
        mixBlendMode: 'multiply',
      }}
    />
  )
}
