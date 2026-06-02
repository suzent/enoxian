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

    const mat4 bayerIndex = mat4(
       0.0/16.0,  8.0/16.0,  2.0/16.0, 10.0/16.0,
      12.0/16.0,  4.0/16.0, 14.0/16.0,  6.0/16.0,
       3.0/16.0, 11.0/16.0,  1.0/16.0,  9.0/16.0,
      15.0/16.0,  7.0/16.0, 13.0/16.0,  5.0/16.0
    );

    void main() {
      vec4 color = texture2D(tDiffuse, vUv) * uExposure;
      float lum  = dot(color.rgb, vec3(0.299, 0.587, 0.114));

      // 2×2 retro pixel cells
      vec2 coord = vUv * uResolution * 0.5;
      int x = int(mod(coord.x, 4.0));
      int y = int(mod(coord.y, 4.0));

      float t = 0.0;
      if      (x==0&&y==0) t = bayerIndex[0][0];
      else if (x==1&&y==0) t = bayerIndex[0][1];
      else if (x==2&&y==0) t = bayerIndex[0][2];
      else if (x==3&&y==0) t = bayerIndex[0][3];
      else if (x==0&&y==1) t = bayerIndex[1][0];
      else if (x==1&&y==1) t = bayerIndex[1][1];
      else if (x==2&&y==1) t = bayerIndex[1][2];
      else if (x==3&&y==1) t = bayerIndex[1][3];
      else if (x==0&&y==2) t = bayerIndex[2][0];
      else if (x==1&&y==2) t = bayerIndex[2][1];
      else if (x==2&&y==2) t = bayerIndex[2][2];
      else if (x==3&&y==2) t = bayerIndex[2][3];
      else if (x==0&&y==3) t = bayerIndex[3][0];
      else if (x==1&&y==3) t = bayerIndex[3][1];
      else if (x==2&&y==3) t = bayerIndex[3][2];
      else if (x==3&&y==3) t = bayerIndex[3][3];

      // Opaque 1-bit: white or black, no gray.
      // Container's mix-blend-mode:multiply makes white transparent.
      float c = step(t, lum);
      gl_FragColor = vec4(vec3(c), 1.0);
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
      ditherPass.uniforms.uResolution.value.set(w, h)
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
export const EXPOSURE_ICON = 1.1
/** Transition peak — fully overexposed = all-white canvas (the "blackout" between circles). */
export const EXPOSURE_TRANSITION_PEAK = 5.0

/** Ease in-out quad helper. */
export function easeInOut(t: number): number {
  return t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t
}
