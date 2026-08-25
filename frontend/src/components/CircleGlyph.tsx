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
  const replaceGlyphRef = useRef<((nextName: string, nextVoided: boolean) => void) | null>(null)

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

    const dc = createDitheredComposer(renderer, scene, camera, size, size)
    dc.setExposure(CIRCLE_EXPOSURE)

    let glyphState: {
      root: THREE.Group
      shape: THREE.Group
      ring: THREE.Mesh | null
      name: string
    } | null = null

    const disposeRoot = (root: THREE.Group) => {
      scene.remove(root)
      root.traverse(object => {
        if (!(object instanceof THREE.Mesh)) return
        object.geometry.dispose()
        const materials = Array.isArray(object.material) ? object.material : [object.material]
        materials.forEach(material => material.dispose())
      })
    }

    replaceGlyphRef.current = (nextName, nextVoided) => {
      if (glyphState) disposeRoot(glyphState.root)

      const root = new THREE.Group()
      const shape = makeCircleGeometry(nextName)
      applyCircleRotation(shape, nextName)
      root.add(shape)

      let ring: THREE.Mesh | null = null
      if (nextVoided) {
        const { smooth } = makeDitherMaterials()
        ring = new THREE.Mesh(new THREE.TorusGeometry(1.28, 0.045, 8, 96), smooth)
        ring.rotation.x = 0.15
        ring.rotation.z = 0.05
        const slash = new THREE.Mesh(new THREE.CylinderGeometry(0.018, 0.018, 3.0, 8), smooth.clone())
        slash.rotation.z = Math.PI / 4
        root.add(ring, slash)
      }

      glyphState = { root, shape, ring, name: nextName }
      scene.add(root)
      dc.composer.render()
    }

    let raf = 0
    let visible = true
    let lastFrame = 0
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    const observer = typeof IntersectionObserver === 'undefined'
      ? null
      : new IntersectionObserver(entries => {
          visible = entries[0]?.isIntersecting ?? true
        })
    observer?.observe(mount)

    const tick = (now: number) => {
      if (!visible || document.hidden) {
        raf = requestAnimationFrame(tick)
        return
      }
      if (now - lastFrame < 1000 / 30) {
        raf = requestAnimationFrame(tick)
        return
      }
      lastFrame = now
      raf = requestAnimationFrame(tick)
      if (glyphState) {
        applyCircleRotation(glyphState.shape, glyphState.name, now)
      }
      if (glyphState?.ring) {
        glyphState.ring.rotation.z += 0.002
        glyphState.ring.scale.setScalar(1 + Math.sin(now * 0.00035) * 0.025)
      }
      dc.composer.render()
    }
    if (reduceMotion) {
      dc.composer.render()
    } else {
      raf = requestAnimationFrame(tick)
    }

    return () => {
      cancelAnimationFrame(raf)
      observer?.disconnect()
      replaceGlyphRef.current = null
      if (glyphState) disposeRoot(glyphState.root)
      dc.composer.dispose()
      renderer.forceContextLoss()
      renderer.dispose()
      renderer.domElement.remove()
    }
  }, [size])

  useEffect(() => {
    replaceGlyphRef.current?.(name, voided)
  }, [name, voided, size])

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
