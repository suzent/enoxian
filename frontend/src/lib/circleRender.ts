import * as THREE from 'three'
import { addDitherLights, EXPOSURE_ICON } from './ditherShader'

export const CIRCLE_CAMERA_FOV = 50
export const CIRCLE_CAMERA_Z = 2.8
export const CIRCLE_EXPOSURE = EXPOSURE_ICON

export function createCircleRenderer(width: number, height: number) {
  const renderer = new THREE.WebGLRenderer({ antialias: false })
  renderer.setPixelRatio(1)
  renderer.setClearColor(0xffffff, 1)
  renderer.setSize(width, height)
  return renderer
}

export function createCircleCamera(aspect = 1, far = 100) {
  const camera = new THREE.PerspectiveCamera(CIRCLE_CAMERA_FOV, aspect, 0.1, far)
  camera.position.z = CIRCLE_CAMERA_Z
  return camera
}

export function prepareCircleScene(scene: THREE.Scene) {
  scene.background = new THREE.Color(0xffffff)
  addDitherLights(scene)
}

export function visibleHeightAtZ(camera: THREE.PerspectiveCamera) {
  return 2 * camera.position.z * Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2)
}

export function objectMaxDimension(shape: THREE.Object3D) {
  const box = new THREE.Box3().setFromObject(shape)
  const size = new THREE.Vector3()
  box.getSize(size)
  return Math.max(size.x, size.y, 0.001)
}

export function rectCenterOnCameraPlane(rect: DOMRect, camera: THREE.PerspectiveCamera) {
  const ndc = new THREE.Vector3(
    ((rect.left + rect.width / 2) / window.innerWidth) * 2 - 1,
    -((rect.top + rect.height / 2) / window.innerHeight) * 2 + 1,
    0.5,
  )
  ndc.unproject(camera)
  const direction = ndc.sub(camera.position).normalize()
  const distance = -camera.position.z / direction.z
  return camera.position.clone().add(direction.multiplyScalar(distance))
}

export function scaleForRect(shape: THREE.Object3D, rect: DOMRect, camera: THREE.PerspectiveCamera) {
  return scaleForRectDimension(objectMaxDimension(shape), rect, camera)
}

export function scaleForRectDimension(maxDimension: number, rect: DOMRect, camera: THREE.PerspectiveCamera) {
  const unitsPerPixel = visibleHeightAtZ(camera) / window.innerHeight
  return (Math.min(rect.width, rect.height) * unitsPerPixel) / maxDimension
}

// ── Dock burst ────────────────────────────────────────────────────────────────

const BURST_COUNT  = 64
const BURST_DUR    = 900    // ms
const GRAVITY      = 5.0    // world units/s²
const BOUNCE_DAMP  = 0.45   // velocity retained after one bounce

/**
 * Spawn a landing-impact burst at `origin` inside `scene`.
 * Particles are placed at negative Z so they render behind the docked object.
 * They scatter in all directions with a bouncy arc — fast radial spread with an
 * upward kick, gravity pull-down, and one Y-bounce off the spawn plane.
 * Returns an updater; call each rAF frame, returns false and self-cleans when done.
 */
export function spawnDockBurst(
  scene: THREE.Scene,
  origin: THREE.Vector3,
): (now: number) => boolean {
  const geo = new THREE.BufferGeometry()
  const pos  = new Float32Array(BURST_COUNT * 3)
  const vx   = new Float32Array(BURST_COUNT)
  const vy   = new Float32Array(BURST_COUNT)
  const vz   = new Float32Array(BURST_COUNT)
  // time at which each particle bounces (when its Y would cross origin.y going down)
  const tBounce = new Float32Array(BURST_COUNT)

  for (let i = 0; i < BURST_COUNT; i++) {
    pos[i * 3]     = origin.x
    pos[i * 3 + 1] = origin.y
    pos[i * 3 + 2] = origin.z

    // Full 360° XZ spread — all directions outward
    const angle   = Math.random() * Math.PI * 2
    const radial  = 1.2 + Math.random() * 3.2
    vx[i] = Math.cos(angle) * radial
    vz[i] = Math.sin(angle) * radial * 0.3  // shallow Z so they stay readable

    // Upward kick — varied so some go high, some stay low
    vy[i] = 0.5 + Math.random() * 2.8

    // Pre-compute bounce time: when vy*t - 0.5*g*t² = 0  →  t = 2*vy/g
    tBounce[i] = (2 * vy[i]) / GRAVITY
  }

  geo.setAttribute('position', new THREE.BufferAttribute(pos, 3))

  const mat = new THREE.PointsMaterial({
    color: 0x222222,
    size: 3.5,
    sizeAttenuation: false,
    transparent: true,
    opacity: 1,
    depthWrite: false,
    depthTest: false,  // always draw, but renderOrder puts them behind the shape
  })

  const points = new THREE.Points(geo, mat)
  points.renderOrder = -1  // render before the docked shape (lower = further back)
  scene.add(points)

  const startTime = performance.now()

  return (now: number): boolean => {
    const elapsed = now - startTime
    const t = elapsed / BURST_DUR

    if (t >= 1) {
      scene.remove(points)
      geo.dispose()
      mat.dispose()
      return false
    }

    const dt = elapsed / 1000
    const posAttr = geo.attributes.position as THREE.BufferAttribute

    for (let i = 0; i < BURST_COUNT; i++) {
      const tb = tBounce[i]
      let y: number

      if (dt < tb) {
        // Pre-bounce arc
        y = origin.y + vy[i] * dt - 0.5 * GRAVITY * dt * dt
      } else {
        // Post-bounce: reflect vy with damping, resume from bounce point (origin.y)
        const dt2 = dt - tb
        const vyB = vy[i] * BOUNCE_DAMP  // speed after bounce
        y = origin.y + vyB * dt2 - 0.5 * GRAVITY * dt2 * dt2
      }

      posAttr.array[i * 3]     = origin.x + vx[i] * dt
      posAttr.array[i * 3 + 1] = y
      // Negative Z offset so particles sit behind objects facing the camera
      posAttr.array[i * 3 + 2] = origin.z - 0.5 + vz[i] * dt
    }
    posAttr.needsUpdate = true

    // Hold full opacity briefly then fade out — emphasises the impact flash
    mat.opacity = t < 0.15 ? 1.0 : Math.pow(1 - (t - 0.15) / 0.85, 1.4)

    return true
  }
}
