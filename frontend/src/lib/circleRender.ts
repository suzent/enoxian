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
