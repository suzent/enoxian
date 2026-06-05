import * as THREE from 'three'

function hashStr(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = Math.imul(31, h) + s.charCodeAt(i) | 0
  return Math.abs(h)
}

/** Return the base geometry for a circle name — one of 18 architectural modes. */
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

  const matWhite = new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 1.0, flatShading: true })
  const matGrey = new THREE.MeshStandardMaterial({ color: 0xdddddd, roughness: 0.8, flatShading: true })
  const matBlack = new THREE.MeshStandardMaterial({ color: 0x999999, roughness: 0.5, flatShading: true })
  const matEmissive = new THREE.MeshBasicMaterial({ color: 0xff0000 })

  function addWireframe(mesh: THREE.Mesh) {
    const edges = new THREE.EdgesGeometry(mesh.geometry)
    const line = new THREE.LineSegments(edges, new THREE.LineBasicMaterial({ color: 0x444444, linewidth: 2 }))
    mesh.add(line)
  }

  function addBlock(w: number, h: number, d: number, x: number, y: number, z: number, mat: THREE.Material): THREE.Mesh {
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), mat)
    mesh.position.set(x, y, z)
    addWireframe(mesh)
    group.add(mesh)
    return mesh
  }

  const isDark = seededRandom() > 0.5
  const mBase = isDark ? matBlack : matWhite
  const mAlt = isDark ? matWhite : matGrey
  const mDetail = seededRandom() > 0.5 ? matBlack : matGrey

  function buildExtrudedBoolean(sz: number, depth: number, forceCorners = -1, forceEdges = -1): THREE.BufferGeometry {
    const shape = new THREE.Shape()
    const r = sz * (0.05 + seededRandom() * 0.15)

    let corners: number[] = [0, 0, 0, 0]
    let edges: boolean[] = [false, false, false, false]

    if (forceCorners !== -1) {
      corners = [forceCorners, forceCorners, forceCorners, forceCorners]
    } else {
      const cCount = 1 + Math.floor(seededRandom() * 2)
      for (let i = 0; i < cCount; i++) {
        corners[Math.floor(seededRandom() * 4)] = Math.floor(seededRandom() * 2) + 1
      }
    }

    if (forceEdges !== -1) {
      edges = [forceEdges === 1, forceEdges === 1, forceEdges === 1, forceEdges === 1]
    } else {
      const eCount = 1 + Math.floor(seededRandom() * 1.5)
      for (let i = 0; i < eCount; i++) {
        edges[Math.floor(seededRandom() * 4)] = true
      }
    }

    if (corners.every(c => c === 0) && edges.every(e => !e)) {
      const mutableCorners: number[] = corners
      mutableCorners[Math.floor(seededRandom() * 4)] = 2
    }

    let cx = -sz / 2, cy = -sz / 2
    if (corners[0] === 0) shape.moveTo(cx, cy)
    else if (corners[0] === 1) shape.moveTo(cx + r, cy)
    else { shape.moveTo(cx, cy + r); shape.absarc(cx, cy, r, Math.PI / 2, 0, true) }

    if (edges[0]) { shape.lineTo(-r * 0.6, -sz / 2); shape.absarc(0, -sz / 2, r * 0.6, Math.PI, 0, true) }

    cx = sz / 2; cy = -sz / 2
    if (corners[1] === 0) shape.lineTo(cx, cy)
    else if (corners[1] === 1) { shape.lineTo(cx - r, cy); shape.lineTo(cx, cy + r) }
    else { shape.lineTo(cx - r, cy); shape.absarc(cx, cy, r, Math.PI, Math.PI / 2, true) }

    if (edges[1]) { shape.lineTo(sz / 2, -r * 0.6); shape.absarc(sz / 2, 0, r * 0.6, -Math.PI / 2, -Math.PI * 1.5, true) }

    cx = sz / 2; cy = sz / 2
    if (corners[2] === 0) shape.lineTo(cx, cy)
    else if (corners[2] === 1) { shape.lineTo(cx, cy - r); shape.lineTo(cx - r, cy) }
    else { shape.lineTo(cx, cy - r); shape.absarc(cx, cy, r, -Math.PI / 2, -Math.PI, true) }

    if (edges[2]) { shape.lineTo(r * 0.6, sz / 2); shape.absarc(0, sz / 2, r * 0.6, 0, -Math.PI, true) }

    cx = -sz / 2; cy = sz / 2
    if (corners[3] === 0) shape.lineTo(cx, cy)
    else if (corners[3] === 1) { shape.lineTo(cx + r, cy); shape.lineTo(cx, cy - r) }
    else { shape.lineTo(cx + r, cy); shape.absarc(cx, cy, r, 0, -Math.PI / 2, true) }

    if (edges[3]) { shape.lineTo(-sz / 2, r * 0.6); shape.absarc(-sz / 2, 0, r * 0.6, Math.PI / 2, -Math.PI / 2, true) }

    cx = -sz / 2; cy = -sz / 2
    if (corners[0] === 0) shape.lineTo(cx, cy)
    else shape.lineTo(cx, cy + r)

    const geo = new THREE.ExtrudeGeometry(shape, { depth, bevelEnabled: false, curveSegments: 32 })
    geo.center()
    return geo
  }

  function getChunkGeometry(type: number, sz: number): THREE.BufferGeometry {
    const shape = new THREE.Shape()
    if (type === 0) return new THREE.BoxGeometry(sz, sz, sz)
    else if (type === 1) {
      shape.moveTo(-sz / 2, -sz / 2); shape.lineTo(sz / 2, -sz / 2)
      shape.lineTo(-sz / 2, sz / 2); shape.lineTo(-sz / 2, -sz / 2)
      const geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false })
      geo.center(); return geo
    } else if (type === 2) {
      shape.moveTo(-sz / 2, -sz / 2); shape.lineTo(sz / 2, -sz / 2)
      shape.absarc(sz / 2, sz / 2, sz, -Math.PI / 2, -Math.PI, true)
      shape.lineTo(-sz / 2, -sz / 2)
      const geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false, curveSegments: 24 })
      geo.center(); return geo
    } else if (type === 3) {
      shape.moveTo(-sz / 2, -sz / 2); shape.lineTo(sz / 2, -sz / 2)
      shape.absarc(-sz / 2, -sz / 2, sz, 0, Math.PI / 2, false)
      shape.lineTo(-sz / 2, -sz / 2)
      const geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false, curveSegments: 24 })
      geo.center(); return geo
    } else {
      const t = sz * 0.3
      shape.moveTo(-sz / 2, -sz / 2); shape.lineTo(sz / 2, -sz / 2)
      shape.lineTo(sz / 2, sz / 2); shape.lineTo(-sz / 2, sz / 2)
      shape.lineTo(-sz / 2, sz / 2 - t); shape.lineTo(sz / 2 - t, sz / 2 - t)
      shape.lineTo(sz / 2 - t, -sz / 2 + t); shape.lineTo(-sz / 2, -sz / 2 + t)
      shape.lineTo(-sz / 2, -sz / 2)
      const geo = new THREE.ExtrudeGeometry(shape, { depth: sz, bevelEnabled: false })
      geo.center(); return geo
    }
  }

  const mode = Math.floor(seededRandom() * 18)

  if (mode === 0) {
    // Menger sponge
    const ninth = baseSize / 9
    const ninthGeo = new THREE.BoxGeometry(ninth, ninth, ninth)
    const ninthEdges = new THREE.EdgesGeometry(ninthGeo)
    const lineMat = new THREE.LineBasicMaterial({ color: 0x444444, linewidth: 2 })
    const glowAxis = Math.floor(seededRandom() * 3)
    for (let x = -4; x <= 4; x++) {
      for (let y = -4; y <= 4; y++) {
        for (let z = -4; z <= 4; z++) {
          const l1x = Math.floor((x + 4) / 3) - 1
          const l1y = Math.floor((y + 4) / 3) - 1
          const l1z = Math.floor((z + 4) / 3) - 1
          const zeros1 = (l1x === 0 ? 1 : 0) + (l1y === 0 ? 1 : 0) + (l1z === 0 ? 1 : 0)
          const l2x = ((x + 4) % 3) - 1
          const l2y = ((y + 4) % 3) - 1
          const l2z = ((z + 4) % 3) - 1
          const zeros2 = (l2x === 0 ? 1 : 0) + (l2y === 0 ? 1 : 0) + (l2z === 0 ? 1 : 0)
          if (zeros1 < 2 && zeros2 < 2) {
            let mat: THREE.Material = mBase
            if (Math.abs(x) === 4 || Math.abs(y) === 4 || Math.abs(z) === 4) mat = mAlt
            if (
              (glowAxis === 0 && Math.abs(x) === 4 && Math.abs(y) <= 1 && Math.abs(z) <= 1) ||
              (glowAxis === 1 && Math.abs(x) <= 1 && Math.abs(y) === 4 && Math.abs(z) <= 1) ||
              (glowAxis === 2 && Math.abs(x) <= 1 && Math.abs(y) <= 1 && Math.abs(z) === 4)
            ) mat = matEmissive
            const mesh = new THREE.Mesh(ninthGeo, mat)
            mesh.position.set(x * ninth, y * ninth, z * ninth)
            mesh.add(new THREE.LineSegments(ninthEdges, lineMat))
            group.add(mesh)
          }
        }
      }
    }
    const coreScale = 0.2 + seededRandom() * 0.4
    addBlock(baseSize * coreScale, baseSize * coreScale, baseSize * coreScale, 0, 0, 0, matWhite)

  } else if (mode === 1) {
    // Cage / scaffolding
    const divs = 2 + Math.floor(seededRandom() * 4)
    const thickRatio = 0.1 + seededRandom() * 0.4
    const step = baseSize / divs
    const thick = step * thickRatio
    const tX = thick, tY = thick * 0.98, tZ = thick * 0.96
    for (let a = 0; a <= divs; a++) {
      for (let b = 0; b <= divs; b++) {
        const pa = -baseSize / 2 + a * step
        const pb = -baseSize / 2 + b * step
        addBlock(baseSize, tX, tX, 0, pa, pb, mAlt)
        addBlock(tY, baseSize, tY, pa, 0, pb, mBase)
        addBlock(tZ, tZ, baseSize, pa, pb, 0, mDetail)
      }
    }
    if (seededRandom() > 0.3) {
      const cap = thick * 1.5
      const capMat = seededRandom() > 0.5 ? matEmissive : mBase
      for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1])
        addBlock(cap, cap, cap, x * baseSize / 2, y * baseSize / 2, z * baseSize / 2, capMat)
    }
    const fillRatio = 0.4 + seededRandom() * 0.4
    addBlock(baseSize * fillRatio, baseSize * fillRatio, baseSize * fillRatio, 0, 0, 0, matBlack)

  } else if (mode === 2) {
    // Void frame
    const t = 0.5 + seededRandom() * 1.5
    const sc = (baseSize / 2) - t / 2
    addBlock(baseSize, t, t, 0, sc, sc, mBase); addBlock(baseSize, t, t, 0, sc, -sc, mBase)
    addBlock(baseSize, t, t, 0, -sc, sc, mBase); addBlock(baseSize, t, t, 0, -sc, -sc, mBase)
    const inner = baseSize - t * 2
    addBlock(t, t, inner, sc, sc, 0, mBase); addBlock(t, t, inner, -sc, sc, 0, mBase)
    addBlock(t, t, inner, sc, -sc, 0, mBase); addBlock(t, t, inner, -sc, -sc, 0, mBase)
    addBlock(t, inner, t, sc, 0, sc, mBase); addBlock(t, inner, t, -sc, 0, sc, mBase)
    addBlock(t, inner, t, sc, 0, -sc, mBase); addBlock(t, inner, t, -sc, 0, -sc, mBase)
    const coreStyle = Math.floor(seededRandom() * 3)
    if (coreStyle === 0) {
      const cs = inner * (0.3 + seededRandom() * 0.4)
      addBlock(cs, cs, cs, 0, 0, 0, mAlt)
    } else if (coreStyle === 1) {
      const beam = inner * 0.2
      addBlock(inner, beam, beam, 0, 0, 0, mAlt); addBlock(beam, inner, beam, 0, 0, 0, mAlt)
      addBlock(beam, beam, inner, 0, 0, 0, mAlt)
      addBlock(beam * 1.5, beam * 1.5, beam * 1.5, 0, 0, 0, matEmissive)
    } else {
      addBlock(inner * 0.6, t * 0.5, inner * 0.6, 0, 0, 0, matWhite)
    }

  } else if (mode === 3) {
    // Stacked slices
    const layers = 3 + Math.floor(seededRandom() * 5)
    const totalSliceH = baseSize * (0.3 + seededRandom() * 0.5)
    const sliceH = totalSliceH / layers
    const gap = (baseSize - totalSliceH) / (layers - 1)
    const profile = Math.floor(seededRandom() * 3)
    for (let i = 0; i < layers; i++) {
      const y = -baseSize / 2 + sliceH / 2 + i * (sliceH + gap)
      let shrink = 0
      if (profile === 1) shrink = (1.0 - Math.abs((layers - 1) / 2 - i) / ((layers - 1) / 2)) * 2.0
      else if (profile === 2) shrink = (Math.abs((layers - 1) / 2 - i) / ((layers - 1) / 2)) * 2.0
      addBlock(baseSize - shrink, sliceH, baseSize - shrink, 0, y, 0, i % 2 === 0 ? mBase : mAlt)
    }
    const spineW = baseSize * (0.1 + seededRandom() * 0.3)
    addBlock(spineW, baseSize, spineW, 0, 0, 0, mDetail)
    if (seededRandom() > 0.5) addBlock(spineW * 1.1, baseSize * 0.3, spineW * 1.1, 0, 0, 0, matEmissive)

  } else if (mode === 4) {
    // Corner matrix
    const cRatio = 0.15 + seededRandom() * 0.25
    const cSize = baseSize * cRatio
    const dist = baseSize / 2 - cSize / 2
    for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1])
      addBlock(cSize, cSize, cSize, x * dist, y * dist, z * dist, mBase)
    const bridgeW = cSize * (0.2 + seededRandom() * 0.8)
    const bridgeL = baseSize - cSize * 2
    for (const y of [-1, 1]) for (const z of [-1, 1]) addBlock(bridgeL, bridgeW, bridgeW, 0, y * dist, z * dist, mAlt)
    for (const x of [-1, 1]) for (const z of [-1, 1]) addBlock(bridgeW, bridgeL, bridgeW, x * dist, 0, z * dist, mAlt)
    for (const x of [-1, 1]) for (const y of [-1, 1]) addBlock(bridgeW, bridgeW, bridgeL, x * dist, y * dist, 0, mAlt)
    const coreS = cSize * (0.8 + seededRandom() * 0.8)
    addBlock(coreS, coreS, coreS, 0, 0, 0, seededRandom() > 0.5 ? matEmissive : mDetail)

  } else if (mode === 5) {
    // Symmetrical boolean mass
    const cs = baseSize / 3
    const rules = [
      { exist: seededRandom() > 0.1, scale: 0.4 + seededRandom() * 0.6, mat: mAlt },
      { exist: seededRandom() > 0.4, scale: 0.3 + seededRandom() * 0.7, mat: mDetail },
      { exist: seededRandom() > 0.3, scale: 0.2 + seededRandom() * 0.8, mat: seededRandom() > 0.7 ? matEmissive : mBase },
    ]
    addBlock(cs, cs, cs, 0, 0, 0, mBase)
    for (let x = -1; x <= 1; x++) for (let y = -1; y <= 1; y++) for (let z = -1; z <= 1; z++) {
      const d = Math.abs(x) + Math.abs(y) + Math.abs(z)
      if (d === 0) continue
      const rule = rules[d - 1]
      if (rule.exist) { const w = cs * rule.scale; addBlock(w, w, w, x * cs, y * cs, z * cs, rule.mat) }
    }

  } else if (mode === 6) {
    // Floating corner nodes
    const thick = baseSize * (0.2 + seededRandom() * 0.2)
    addBlock(baseSize, thick, thick, 0, 0, 0, mBase)
    addBlock(thick, baseSize, thick, 0, 0, 0, mBase)
    addBlock(thick, thick, baseSize, 0, 0, 0, mBase)
    const voidSize = (baseSize - thick) / 2
    const nodeS = voidSize * (0.4 + seededRandom() * 0.4)
    const dist = baseSize / 2 - voidSize / 2
    for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1])
      addBlock(nodeS, nodeS, nodeS, x * dist, y * dist, z * dist, mAlt)
    addBlock(thick * 1.1, thick * 1.1, thick * 1.1, 0, 0, 0, matEmissive)

  } else if (mode === 7) {
    // Cross-punched monolith
    const cS = baseSize * (0.3 + seededRandom() * 0.1)
    const dist = baseSize / 2 - cS / 2
    for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1])
      addBlock(cS, cS, cS, x * dist, y * dist, z * dist, mBase)
    addBlock(baseSize * 0.65, baseSize * 0.65, baseSize * 0.65, 0, 0, 0, mAlt)
    if (seededRandom() > 0.3) {
      const bridgeT = baseSize * 0.15
      addBlock(baseSize, bridgeT, bridgeT, 0, 0, 0, matEmissive)
      addBlock(bridgeT, baseSize, bridgeT, 0, 0, 0, matEmissive)
      addBlock(bridgeT, bridgeT, baseSize, 0, 0, 0, matEmissive)
    }

  } else if (mode === 8) {
    // Intersecting hollow frames
    const fW = baseSize
    const fT = baseSize * (0.1 + seededRandom() * 0.15)
    const gap = fW - fT * 2
    const t0 = fT, t1 = fT * 0.98, t2 = fT * 0.96
    addBlock(fW, t0, t0, 0, fW / 2 - fT / 2, 0, mBase); addBlock(fW, t0, t0, 0, -fW / 2 + fT / 2, 0, mBase)
    addBlock(t0, gap, t0, -fW / 2 + fT / 2, 0, 0, mBase); addBlock(t0, gap, t0, fW / 2 - fT / 2, 0, 0, mBase)
    addBlock(fW, t1, t1, 0, 0, fW / 2 - fT / 2, mAlt); addBlock(fW, t1, t1, 0, 0, -fW / 2 + fT / 2, mAlt)
    addBlock(t1, t1, gap, -fW / 2 + fT / 2, 0, 0, mAlt); addBlock(t1, t1, gap, fW / 2 - fT / 2, 0, 0, mAlt)
    addBlock(t2, fW, t2, 0, 0, fW / 2 - fT / 2, mDetail); addBlock(t2, fW, t2, 0, 0, -fW / 2 + fT / 2, mDetail)
    addBlock(t2, gap, t2, 0, fW / 2 - fT / 2, 0, mDetail); addBlock(t2, gap, t2, 0, -fW / 2 + fT / 2, 0, mDetail)
    if (seededRandom() > 0.5) addBlock(fT * 1.8, fT * 1.8, fT * 1.8, 0, 0, 0, matEmissive)

  } else if (mode === 9) {
    // Interwoven slat matrix
    const layers = 5 + Math.floor(seededRandom() * 4)
    const slatH = baseSize / layers
    for (let i = 0; i < layers; i++) {
      const y = -baseSize / 2 + slatH / 2 + i * slatH
      const dirX = i % 2 === 0
      const slatCount = 3 + Math.floor(seededRandom() * 3)
      const slatW = baseSize / (slatCount * 2 - 1)
      let lMat: THREE.Material = i % 2 === 0 ? mBase : mAlt
      if (i === 0 || i === layers - 1) lMat = mDetail
      for (let j = 0; j < slatCount; j++) {
        const offset = -baseSize / 2 + slatW / 2 + j * (slatW * 2)
        if (dirX) addBlock(baseSize, slatH * 0.9, slatW * 0.9, 0, y, offset, lMat)
        else addBlock(slatW * 0.9, slatH * 0.9, baseSize, offset, y, 0, lMat)
      }
    }

  } else if (mode === 10) {
    // Curvilinear / boolean monolith
    const geo = buildExtrudedBoolean(baseSize, baseSize)
    const mesh = new THREE.Mesh(geo, mBase)
    mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI / 2)
    mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI / 2)
    mesh.rotation.z = Math.floor(seededRandom() * 4) * (Math.PI / 2)
    addWireframe(mesh); group.add(mesh)
    const coreRad = baseSize * (0.15 + seededRandom() * 0.1)
    const coreType = Math.floor(seededRandom() * 3)
    let coreGeo: THREE.BufferGeometry
    if (coreType === 0) coreGeo = new THREE.SphereGeometry(coreRad, 16, 16)
    else if (coreType === 1) coreGeo = new THREE.CylinderGeometry(coreRad, coreRad, baseSize * 1.1, 16)
    else coreGeo = new THREE.OctahedronGeometry(coreRad * 1.3, 0)
    const core = new THREE.Mesh(coreGeo, matEmissive)
    if (coreType === 1) {
      core.rotation.x = Math.floor(seededRandom() * 3) * (Math.PI / 2)
      core.rotation.z = Math.floor(seededRandom() * 3) * (Math.PI / 2)
    }
    addWireframe(core); group.add(core)

  } else if (mode === 11) {
    // Arc-channel core
    const geo = buildExtrudedBoolean(baseSize, baseSize, 2, -1)
    const mesh = new THREE.Mesh(geo, mAlt)
    mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI / 2)
    addWireframe(mesh); group.add(mesh)
    const ring = new THREE.Mesh(new THREE.TorusGeometry(baseSize * 0.55, 0.2, 4, 16), matEmissive)
    ring.rotation.x = Math.PI / 2
    group.add(ring)

  } else if (mode === 12) {
    // Wedge / chamfer architecture
    const geo = buildExtrudedBoolean(baseSize, baseSize, 1, 0)
    const mesh = new THREE.Mesh(geo, mBase)
    mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI / 2)
    addWireframe(mesh); group.add(mesh)
    const slice = new THREE.Mesh(new THREE.BoxGeometry(baseSize * 1.2, baseSize * 0.1, baseSize * 1.2), mDetail)
    addWireframe(slice); group.add(slice)

  } else if (mode === 13) {
    // U-channel labyrinth
    const geo = buildExtrudedBoolean(baseSize, baseSize, 0, 1)
    const mesh = new THREE.Mesh(geo, mDetail)
    addWireframe(mesh); group.add(mesh)
    group.add(new THREE.Mesh(new THREE.SphereGeometry(baseSize * 0.25, 16, 16), matEmissive))

  } else if (mode === 14) {
    // Sliced boolean fins
    const depth = baseSize * (0.05 + seededRandom() * 0.1)
    const geo = buildExtrudedBoolean(baseSize, depth)
    const layers = 4 + Math.floor(seededRandom() * 5)
    const step = baseSize / layers
    for (let i = 0; i < layers; i++) {
      const m = new THREE.Mesh(geo, i % 2 === 0 ? mBase : mAlt)
      m.position.z = -baseSize / 2 + step / 2 + i * step
      if (seededRandom() > 0.85) m.rotation.z = Math.PI / 2
      addWireframe(m); group.add(m)
    }
    const spine = new THREE.Mesh(new THREE.CylinderGeometry(baseSize * 0.1, baseSize * 0.1, baseSize, 16), matEmissive)
    spine.rotation.x = Math.PI / 2
    group.add(spine)

  } else if (mode === 15) {
    // Omni-directional 2x2x2 chunk assembly
    const sz = baseSize / 2
    const dist = sz / 2
    for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1]) {
      if (seededRandom() > 0.8) continue
      const type = Math.floor(seededRandom() * 5)
      const geo = getChunkGeometry(type, sz)
      let cMat: THREE.Material = x * y * z > 0 ? mBase : mAlt
      if (seededRandom() > 0.8) cMat = mDetail
      const mesh = new THREE.Mesh(geo, cMat)
      mesh.position.set(x * dist, y * dist, z * dist)
      mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      mesh.rotation.z = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      addWireframe(mesh); group.add(mesh)
      if (type !== 0 && seededRandom() > 0.7) {
        const coreS = sz * 0.25
        const em = new THREE.Mesh(new THREE.BoxGeometry(coreS, coreS, coreS), matEmissive)
        em.position.copy(mesh.position); group.add(em)
      }
    }

  } else if (mode === 16) {
    // Omni-directional 3x3x3 grid
    const sz = baseSize / 3
    const offset = sz
    for (let x = -1; x <= 1; x++) for (let y = -1; y <= 1; y++) for (let z = -1; z <= 1; z++) {
      const distC = Math.abs(x) + Math.abs(y) + Math.abs(z)
      if (distC === 0 && seededRandom() > 0.3) {
        group.add(new THREE.Mesh(new THREE.SphereGeometry(sz * 0.8, 16, 16), matEmissive))
        continue
      }
      if (seededRandom() > 0.5) continue
      const type = Math.floor(seededRandom() * 5)
      const geo = getChunkGeometry(type, sz)
      const mesh = new THREE.Mesh(geo, distC % 2 === 0 ? mBase : mAlt)
      mesh.position.set(x * offset, y * offset, z * offset)
      mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      mesh.rotation.z = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      addWireframe(mesh); group.add(mesh)
    }

  } else {
    // Multi-axis intersecting beams
    for (let axis = 0; axis < 3; axis++) {
      const pS = baseSize * (0.3 + seededRandom() * 0.2)
      let type = Math.floor(seededRandom() * 5)
      if (type === 0) type = 4
      const geo = getChunkGeometry(type, pS) as THREE.BufferGeometry & { scale(x: number, y: number, z: number): void }
      geo.scale(1, 1, baseSize / pS)
      const mat: THREE.Material = axis === 0 ? mBase : axis === 1 ? mAlt : mDetail
      const mesh = new THREE.Mesh(geo, mat)
      if (axis === 0) { mesh.rotation.y = Math.PI / 2; mesh.rotation.x = Math.floor(seededRandom() * 4) * (Math.PI / 2) }
      if (axis === 1) { mesh.rotation.x = Math.PI / 2; mesh.rotation.y = Math.floor(seededRandom() * 4) * (Math.PI / 2) }
      if (axis === 2) mesh.rotation.z = Math.floor(seededRandom() * 4) * (Math.PI / 2)
      addWireframe(mesh); group.add(mesh)
    }
    const emMesh = new THREE.Mesh(new THREE.OctahedronGeometry(baseSize * 0.25, 0), matEmissive)
    addWireframe(emMesh); group.add(emMesh)
  }

  // Fallback if completely empty
  if (group.children.length === 0) {
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(baseSize * 0.6, baseSize * 0.6, baseSize * 0.6), mBase)
    addWireframe(mesh); group.add(mesh)
  }

  // Re-center
  const box = new THREE.Box3().setFromObject(group)
  const center = new THREE.Vector3()
  box.getCenter(center)
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

export function circleRotationAt(name: string, timeMs = performance.now()) {
  const params = makeShapeParams(name)
  return {
    x: params.initRotX + params.rotX * timeMs * 0.06,
    y: params.initRotY + params.rotY * timeMs * 0.06,
    z: params.rotZ * timeMs * 0.06,
  }
}

export function applyCircleRotation(target: THREE.Object3D, name: string, timeMs = performance.now()) {
  const rotation = circleRotationAt(name, timeMs)
  target.rotation.set(rotation.x, rotation.y, rotation.z)
}
