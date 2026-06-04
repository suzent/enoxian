import * as THREE from 'three'

function hashStr(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = Math.imul(31, h) + s.charCodeAt(i) | 0
  return Math.abs(h)
}

/** Return the base geometry for a circle name — one of the 5 Platonic solids. */
export function makeCircleGeometry(name: string): THREE.Group {
  const sInit = hashStr(name)
  let s = sInit

  function seededRandom() {
    let t = s + 0x6D2B79F5
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    s++
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }

  const group = new THREE.Group()

  const baseSize = 1.4
  const matWhite = new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 1.0 })
  const matGrey = new THREE.MeshStandardMaterial({ color: 0xdddddd, roughness: 0.8 })
  const matBlack = new THREE.MeshStandardMaterial({ color: 0x999999, roughness: 0.5 })
  const matEmissive = new THREE.MeshBasicMaterial({ color: 0xff0000 })

  function addWireframe(mesh: THREE.Mesh) {
    const edges = new THREE.EdgesGeometry(mesh.geometry)
    const line = new THREE.LineSegments(edges, new THREE.LineBasicMaterial({ color: 0x444444, linewidth: 2 }))
    mesh.add(line)
  }

  function getChunkGeometry(type: number, sz: number) {
    let geo
    const shape = new THREE.Shape()
    if (type === 0) {
        return new THREE.BoxGeometry(sz, sz, sz)
    } else if (type === 1) { // Wedge
        shape.moveTo(-sz/2, -sz/2)
        shape.lineTo(sz/2, -sz/2)
        shape.lineTo(-sz/2, sz/2)
        shape.lineTo(-sz/2, -sz/2)
        geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false })
        geo.center()
        return geo
    } else if (type === 2) { // Scoop
        shape.moveTo(-sz/2, -sz/2)
        shape.lineTo(sz/2, -sz/2)
        shape.absarc(sz/2, sz/2, sz, -Math.PI/2, -Math.PI, true)
        shape.lineTo(-sz/2, -sz/2)
        geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false, curveSegments: 24 })
        geo.center()
        return geo
    } else if (type === 3) { // Rounded
        shape.moveTo(-sz/2, -sz/2)
        shape.lineTo(sz/2, -sz/2)
        shape.absarc(-sz/2, -sz/2, sz, 0, Math.PI/2, false)
        shape.lineTo(-sz/2, -sz/2)
        geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false, curveSegments: 24 })
        geo.center()
        return geo
    } else { // C-cut
        const t = sz * 0.3
        shape.moveTo(-sz/2, -sz/2)
        shape.lineTo(sz/2, -sz/2)
        shape.lineTo(sz/2, sz/2)
        shape.lineTo(-sz/2, sz/2)
        shape.lineTo(-sz/2, sz/2 - t)
        shape.lineTo(sz/2 - t, sz/2 - t)
        shape.lineTo(sz/2 - t, -sz/2 + t)
        shape.lineTo(-sz/2, -sz/2 + t)
        shape.lineTo(-sz/2, -sz/2)
        geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false })
        geo.center()
        return geo
    }
  }

  const isDark = seededRandom() > 0.5
  const mBase = isDark ? matBlack : matWhite
  const mAlt = isDark ? matWhite : matGrey
  const mDetail = (seededRandom() > 0.5) ? matBlack : matGrey

  // We use Mode 15 (Omni-Directional 2x2x2) and 16 (3x3x3 Grid) logic simplified for the frontend
  const useHighDensity = seededRandom() > 0.5
  
  if (useHighDensity) {
    // Mode 16 style: 3x3x3 Grid
    const sz = baseSize / 3
    const offset = sz
    
    for (const x of [-1, 0, 1]) {
        for (const y of [-1, 0, 1]) {
            for (const z of [-1, 0, 1]) {
                const distC = Math.abs(x) + Math.abs(y) + Math.abs(z)
                if (distC === 0 && seededRandom() > 0.3) {
                    const coreGeo = new THREE.SphereGeometry(sz*0.8, 16, 16)
                    const core = new THREE.Mesh(coreGeo, matEmissive)
                    group.add(core)
                    continue
                }
                
                if (seededRandom() > 0.5) continue 
                
                const type = Math.floor(seededRandom() * 5)
                const geo = getChunkGeometry(type, sz)
                
                const cMat = (distC % 2 === 0) ? mBase : mAlt
                
                const mesh = new THREE.Mesh(geo, cMat)
                mesh.position.set(x * offset, y * offset, z * offset)
                
                mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI/2)
                mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI/2)
                mesh.rotation.z = Math.floor(seededRandom() * 4) * (Math.PI/2)
                
                addWireframe(mesh)
                group.add(mesh)
            }
        }
    }
  } else {
    // Mode 15 style: 2x2x2 Chunk Assembly
    const sz = baseSize / 2
    const dist = sz / 2
    
    for (const x of [-1, 1]) {
        for (const y of [-1, 1]) {
            for (const z of [-1, 1]) {
                if (seededRandom() > 0.8) continue 
                
                const type = Math.floor(seededRandom() * 5)
                const geo = getChunkGeometry(type, sz)
                
                let cMat = (x*y*z > 0) ? mBase : mAlt
                if (seededRandom() > 0.8) cMat = mDetail
                
                const mesh = new THREE.Mesh(geo, cMat)
                mesh.position.set(x * dist, y * dist, z * dist)
                
                mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI/2)
                mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI/2)
                mesh.rotation.z = Math.floor(seededRandom() * 4) * (Math.PI/2)
                
                addWireframe(mesh)
                group.add(mesh)
                
                if (type !== 0 && seededRandom() > 0.7) {
                    const coreS = sz * 0.25
                    const em = new THREE.Mesh(new THREE.BoxGeometry(coreS, coreS, coreS), matEmissive)
                    em.position.copy(mesh.position)
                    group.add(em)
                }
            }
        }
    }
  }

  // Fallback if completely empty
  if (group.children.length === 0) {
      const geo = new THREE.BoxGeometry(baseSize*0.6, baseSize*0.6, baseSize*0.6)
      const mesh = new THREE.Mesh(geo, mBase)
      addWireframe(mesh)
      group.add(mesh)
  }

  // Re-center
  const newBox = new THREE.Box3().setFromObject(group)
  const center = new THREE.Vector3()
  newBox.getCenter(center)
  group.position.sub(center)

  return group
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
