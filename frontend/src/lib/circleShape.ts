import * as THREE from 'three'

function hashStr(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = Math.imul(31, h) + s.charCodeAt(i) | 0
  return Math.abs(h)
}

/** Return the base geometry for a circle name — one of the 5 Platonic solids. */
export function makeCircleGeometry(name: string): THREE.BufferGeometry {
  const h = hashStr(name)
  switch (h % 5) {
    case 0: return new THREE.TetrahedronGeometry(1, 0)
    case 1: return new THREE.OctahedronGeometry(1, 0)
    case 2: return new THREE.BoxGeometry(1.4, 1.4, 1.4)
    case 3: return new THREE.IcosahedronGeometry(1, 0)
    default: return new THREE.DodecahedronGeometry(1, 0)
  }
}

export interface ShapeParams {
  initRotX: number
  initRotY: number
  rotX: number
  rotY: number
  rotZ: number
}

/** Deterministic rotation speed + initial pose from name hash. */
export function makeShapeParams(name: string): ShapeParams {
  const h = hashStr(name)
  return {
    initRotX: ((h & 0xff) / 255) * Math.PI * 2,
    initRotY: (((h >> 8) & 0xff) / 255) * Math.PI * 2,
    rotX: 0.004 + ((h >> 10) & 0xf) * 0.0003,
    rotY: 0.006 + ((h >> 14) & 0xf) * 0.0004,
    rotZ: ((h >> 18) & 1) ? 0.002 : 0,
  }
}
