import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import { EffectComposer } from 'three/examples/jsm/postprocessing/EffectComposer.js'
import { RenderPass } from 'three/examples/jsm/postprocessing/RenderPass.js'
import { ShaderPass } from 'three/examples/jsm/postprocessing/ShaderPass.js'
import { makeCircleGeometry, makeShapeParams } from '../lib/circleShape'

// Bayer 4×4 ordered dither — same shader as the original demo.
// Outputs opaque 1-bit black/white; the mount div uses mix-blend-mode:multiply
// so white = transparent over the app UI and black = visible dots.
// uExposure fades from high (overexposed → few dots) to target → used for fade-in.
const DitherShader = {
  uniforms: {
    tDiffuse:    { value: null as THREE.Texture | null },
    uResolution: { value: new THREE.Vector2() },
    uExposure:   { value: 3.0 }, // starts overexposed (bright/invisible), ramps down
  },
  vertexShader: `
    varying vec2 vUv;
    void main() {
      vUv = uv;
      gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    }
  `,
  fragmentShader: `
    uniform sampler2D tDiffuse;
    uniform vec2 uResolution;
    uniform float uExposure;
    varying vec2 vUv;

    const mat4 bayerIndex = mat4(
       0.0/16.0,  8.0/16.0,  2.0/16.0, 10.0/16.0,
      12.0/16.0,  4.0/16.0, 14.0/16.0,  6.0/16.0,
       3.0/16.0, 11.0/16.0,  1.0/16.0,  9.0/16.0,
      15.0/16.0,  7.0/16.0, 13.0/16.0,  5.0/16.0
    );

    void main() {
      vec4 color = texture2D(tDiffuse, vUv) * uExposure;
      float lum = dot(color.rgb, vec3(0.299, 0.587, 0.114));

      // 2×2 retro pixel cells (same scale as the demo)
      vec2 coord = vUv * uResolution * 0.5;
      int x = int(mod(coord.x, 4.0));
      int y = int(mod(coord.y, 4.0));

      float threshold = 0.0;
      if      (x==0&&y==0) threshold = bayerIndex[0][0];
      else if (x==1&&y==0) threshold = bayerIndex[0][1];
      else if (x==2&&y==0) threshold = bayerIndex[0][2];
      else if (x==3&&y==0) threshold = bayerIndex[0][3];
      else if (x==0&&y==1) threshold = bayerIndex[1][0];
      else if (x==1&&y==1) threshold = bayerIndex[1][1];
      else if (x==2&&y==1) threshold = bayerIndex[1][2];
      else if (x==3&&y==1) threshold = bayerIndex[1][3];
      else if (x==0&&y==2) threshold = bayerIndex[2][0];
      else if (x==1&&y==2) threshold = bayerIndex[2][1];
      else if (x==2&&y==2) threshold = bayerIndex[2][2];
      else if (x==3&&y==2) threshold = bayerIndex[2][3];
      else if (x==0&&y==3) threshold = bayerIndex[3][0];
      else if (x==1&&y==3) threshold = bayerIndex[3][1];
      else if (x==2&&y==3) threshold = bayerIndex[3][2];
      else if (x==3&&y==3) threshold = bayerIndex[3][3];

      // Opaque 1-bit output — white or black, no gray.
      // mix-blend-mode:multiply makes white transparent over the UI.
      float c = step(threshold, lum);
      gl_FragColor = vec4(vec3(c), 1.0);
    }
  `,
}

interface Props {
  circleName: string
}

export default function VoidOverlay({ circleName }: Props) {
  const mountRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const mount = mountRef.current
    if (!mount) return

    const W = window.innerWidth
    const H = window.innerHeight

    // Solid white background — unlit areas dither to white (transparent via multiply)
    const renderer = new THREE.WebGLRenderer({ antialias: false })
    renderer.setSize(W, H)
    renderer.setPixelRatio(1) // 1:1 for correct Bayer cell mapping
    renderer.setClearColor(0xffffff, 1)
    mount.appendChild(renderer.domElement)

    const scene = new THREE.Scene()
    scene.background = new THREE.Color(0xffffff)

    const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 100)
    camera.position.z = 9

    // Mirror the demo's lighting — ambient 0.4 + single directional.
    // flatShading + this setup creates clear face-to-face luminance jumps
    // that the Bayer dither turns into distinct halftone regions.
    scene.add(new THREE.AmbientLight(0xffffff, 0.4))
    const key = new THREE.DirectionalLight(0xffffff, 1.2)
    key.position.set(5, 10, 15)
    scene.add(key)
    const back = new THREE.DirectionalLight(0xffffff, 0.3)
    back.position.set(-8, -5, -4)
    scene.add(back)

    // Mid-gray, not black — pure black absorbs all light and kills dither variation.
    // 0x888888 gives luminance range ~0.2 (shadow) → ~0.75 (highlight),
    // which maps to clearly distinct Bayer dot densities.
    const shapeMat = new THREE.MeshPhongMaterial({ color: 0x888888, flatShading: true })
    const smoothMat = new THREE.MeshPhongMaterial({ color: 0x777777, flatShading: false })

    // Circle's unique Platonic solid — solid mesh so flat-shading creates
    // sharp per-face luminance steps → distinct dither bands per face
    const baseGeo = makeCircleGeometry(circleName)
    const shape = new THREE.Mesh(baseGeo, shapeMat)
    const params = makeShapeParams(circleName)
    shape.rotation.x = params.initRotX
    shape.rotation.y = params.initRotY
    shape.scale.setScalar(2.6)
    scene.add(shape)

    // Large outer torus — the circular part of ∅, smooth-shaded for gradient dither bands
    const ring = new THREE.Mesh(
      new THREE.TorusGeometry(5.2, 0.14, 8, 128),
      smoothMat
    )
    ring.rotation.x = 0.15
    ring.rotation.z = 0.05
    scene.add(ring)

    // Diagonal slash — the ∅ strike-through
    const slash = new THREE.Mesh(
      new THREE.CylinderGeometry(0.045, 0.045, 11.5, 8),
      smoothMat.clone()
    )
    slash.rotation.z = Math.PI / 4
    scene.add(slash)

    // ── EffectComposer with Bayer dither pass ────────────────────────────────
    const composer = new EffectComposer(renderer)
    composer.addPass(new RenderPass(scene, camera))

    const ditherPass = new ShaderPass(DitherShader)
    ditherPass.uniforms.uResolution.value.set(W, H)
    composer.addPass(ditherPass)

    // ── Animation ─────────────────────────────────────────────────────────────
    let raf = 0
    const startTime = performance.now()
    const FADE_MS = 1000
    const EXPOSURE_START = 4.0   // overexposed → mostly white → no dots at fade-in start
    const EXPOSURE_TARGET = 1.35 // slightly over neutral so highlights stay transparent

    const tick = () => {
      raf = requestAnimationFrame(tick)

      // Fade in by ramping exposure DOWN from bright to target
      const t = Math.min((performance.now() - startTime) / FADE_MS, 1)
      const eased = t < 0.5 ? 2*t*t : -1+(4-2*t)*t
      ditherPass.uniforms.uExposure.value = EXPOSURE_START - (EXPOSURE_START - EXPOSURE_TARGET) * eased

      shape.rotation.x += params.rotX * 0.22
      shape.rotation.y += params.rotY * 0.22
      ring.rotation.z += 0.0007
      ring.scale.setScalar(1 + Math.sin(performance.now() * 0.00035) * 0.025)

      composer.render()
    }
    tick()

    const onResize = () => {
      const nW = window.innerWidth, nH = window.innerHeight
      renderer.setSize(nW, nH)
      composer.setSize(nW, nH)
      camera.aspect = nW / nH
      camera.updateProjectionMatrix()
      ditherPass.uniforms.uResolution.value.set(nW, nH)
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

  // mix-blend-mode:multiply — white dither pixels vanish over the UI,
  // black dither pixels show as black dots. No alpha hacks needed.
  return (
    <div
      ref={mountRef}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 60,
        pointerEvents: 'none',
        mixBlendMode: 'multiply',
      }}
    />
  )
}
