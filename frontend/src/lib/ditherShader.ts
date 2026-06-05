/**
 * Universal Bayer 4×4 ordered dither for the app's 3D elements.
 * Used by: VoidOverlay, CircleSidebar icons, circle-switch transition.
 *
 * Render pipeline:
 *   THREE.Scene → RenderPass → DitherPass → canvas
 *
 * White pixels become transparent via CSS mix-blend-mode:multiply on the
 * canvas container. Dark pixels appear as crisp dither dots.
 */
import * as THREE from 'three'
import { EffectComposer } from 'three/examples/jsm/postprocessing/EffectComposer.js'
import { RenderPass } from 'three/examples/jsm/postprocessing/RenderPass.js'
import { ShaderPass } from 'three/examples/jsm/postprocessing/ShaderPass.js'

// ── Shader ────────────────────────────────────────────────────────────────────

export const DitherShaderDef = {
  uniforms: {
    tDiffuse:    { value: null as THREE.Texture | null },
    uResolution: { value: new THREE.Vector2() },
    // Exposure controls dither density: high = overexposed = few dots,
    // low = underexposed = many dots.  Animate for fade-in/out effects.
    uExposure:   { value: 1.35 },
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

    float bayer(vec2 p) {
        vec2 b = floor(mod(p, 4.0));
        int idx = int(b.x) + int(b.y) * 4;
        if(idx == 0) return 0.0/16.0;
        if(idx == 1) return 8.0/16.0;
        if(idx == 2) return 2.0/16.0;
        if(idx == 3) return 10.0/16.0;
        if(idx == 4) return 12.0/16.0;
        if(idx == 5) return 4.0/16.0;
        if(idx == 6) return 14.0/16.0;
        if(idx == 7) return 6.0/16.0;
        if(idx == 8) return 3.0/16.0;
        if(idx == 9) return 11.0/16.0;
        if(idx == 10) return 1.0/16.0;
        if(idx == 11) return 9.0/16.0;
        if(idx == 12) return 15.0/16.0;
        if(idx == 13) return 7.0/16.0;
        if(idx == 14) return 13.0/16.0;
        return 5.0/16.0;
    }

    void main() {
      // 1. Get raw scene color
      vec4 rawColor = texture2D(tDiffuse, vUv);
      
      // 2. Apply exposure early so it scales the light intensity uniformly
      // Removed the clamp entirely so overexposure can push colors far beyond 1.0
      vec4 color = rawColor * uExposure;

      // Increase contrast aggressively before dithering but keep it softer than smoothstep
      vec3 contrastColor = (color.rgb - 0.5) * 1.5 + 0.5;
      float gray = dot(contrastColor, vec3(0.299, 0.587, 0.114));
      
      // Add slight noise based on UV to break up banding before bayer pattern
      float noise = fract(sin(dot(vUv, vec2(12.9898, 78.233))) * 43758.5453) * 0.05 - 0.025;
      gray = clamp(gray + noise, 0.0, 1.0);
      
      float pixelSize = 1.0;
      vec2 pixelCoord = floor((vUv * uResolution) / pixelSize);
      
      float threshold = bayer(pixelCoord);
      
      // Check for emissive Red (bright red, low green/blue)
      bool isRed = (color.r > 0.5 && color.g < 0.3 && color.b < 0.3);
      
      // EXCEPTION 1: Pure White Background Bypass
      float distToWhite = length(color.rgb - vec3(1.0));
      if (distToWhite < 0.05 && !isRed) {
          gl_FragColor = vec4(1.0, 1.0, 1.0, 1.0);
          return;
      }

      if (isRed) {
          float redDither = step(threshold, color.r * 1.2);
          gl_FragColor = vec4(redDither, 0.0, 0.0, 1.0);
      } else {
          // White background: high gray → white (transparent via multiply), low gray → black dot
          float bw = step(threshold, gray);
          gl_FragColor = vec4(vec3(bw), 1.0);
      }
    }
  `,
}

// ── Composer factory ──────────────────────────────────────────────────────────

export interface DitheredComposer {
  composer: EffectComposer
  ditherPass: ShaderPass
  setSize(w: number, h: number): void
  /** Convenience: set exposure (high = few dots, low = many dots) */
  setExposure(v: number): void
}

/**
 * Create an EffectComposer with the Bayer dither pass attached.
 * Each call produces independent uniforms so multiple composers don't share state.
 */
export function createDitheredComposer(
  renderer: THREE.WebGLRenderer,
  scene: THREE.Scene,
  camera: THREE.Camera,
  width: number,
  height: number,
): DitheredComposer {
  const composer = new EffectComposer(renderer)
  composer.addPass(new RenderPass(scene, camera))

  // Clone uniforms so each composer instance has its own values
  const ditherPass = new ShaderPass({
    ...DitherShaderDef,
    uniforms: {
      tDiffuse:    { value: null },
      uResolution: { value: new THREE.Vector2(width, height) },
      uExposure:   { value: DitherShaderDef.uniforms.uExposure.value },
    },
  })
  composer.addPass(ditherPass)

  return {
    composer,
    ditherPass,
    setSize(w, h) {
      composer.setSize(w, h)
      ditherPass.uniforms.uResolution.value.set(w * window.devicePixelRatio, h * window.devicePixelRatio)
    },
    setExposure(v) {
      ditherPass.uniforms.uExposure.value = v
    },
  }
}

// ── Standard scene setup ──────────────────────────────────────────────────────

/** Add the standard dither lighting to a scene. */
export function addDitherLights(scene: THREE.Scene): void {
  scene.add(new THREE.AmbientLight(0xffffff, 0.4))
  const key = new THREE.DirectionalLight(0xffffff, 1.2)
  key.position.set(5, 10, 15)
  scene.add(key)
  const back = new THREE.DirectionalLight(0xffffff, 0.3)
  back.position.set(-8, -5, -4)
  scene.add(back)
}

/** Standard materials: mid-gray so lighting creates visible dither gradients. */
export function makeDitherMaterials() {
  return {
    /** Flat-shaded: sharp per-face luminance jumps → distinct dither bands. */
    flat:   new THREE.MeshPhongMaterial({ color: 0x888888, flatShading: true }),
    /** Smooth-shaded: gradient → banded dither rings on curved surfaces. */
    smooth: new THREE.MeshPhongMaterial({ color: 0x777777, flatShading: false }),
  }
}

// ── Exposure constants ────────────────────────────────────────────────────────

/** Overexposed start — used at the beginning of fade-in so nothing is visible yet. */
export const EXPOSURE_FADE_START = 4.0
/** Target exposure for the void overlay (subtle ambient dither effect). */
export const EXPOSURE_VOID = 2.4
/** Target exposure for icons (slightly denser to show shape at small size). */
export const EXPOSURE_ICON = 1.45
/** Transition peak — fully overexposed = all-white canvas (the "blackout" between circles). */
export const EXPOSURE_TRANSITION_PEAK = 5.0

/** Ease in-out quad helper. */
export function easeInOut(t: number): number {
  return t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t
}
