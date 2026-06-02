import * as THREE from 'three'
import { createDitheredComposer } from '../lib/ditherShader'

export type AnimState = 'idle' | 'inscription' | 'erupting' | 'breathing' | 'revelation' | 'ingress' | 'fading' | 'done'

export interface AngelScene {
  triggerEruption(onComplete: () => void): void
  dispose(): void
}

export function buildAngelScene(mount: HTMLDivElement): AngelScene {
  const W = window.innerWidth
  const H = window.innerHeight

  const renderer = new THREE.WebGLRenderer({ antialias: false })
  renderer.setSize(W, H)
  renderer.setPixelRatio(1)
  renderer.setClearColor(0xffffff, 1)
  mount.appendChild(renderer.domElement)

  const scene = new THREE.Scene()
  scene.background = new THREE.Color(0xffffff)
  
  const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 150)
  const baseCamPos = new THREE.Vector3(0, -4, 34)
  camera.position.copy(baseCamPos)
  
  const baseLookTarget = new THREE.Vector3(0, 0, 0)
  const currentLookTarget = new THREE.Vector3(0, 0, 0)
  camera.lookAt(baseLookTarget)

  scene.add(new THREE.AmbientLight(0xffffff, 0.4))
  const keyLight = new THREE.DirectionalLight(0xffffff, 3.5)
  keyLight.position.set(-10, 15, 12)
  scene.add(keyLight)
  const fillLight = new THREE.DirectionalLight(0xffffff, 0.5)
  fillLight.position.set(10, 5, 2)
  scene.add(fillLight)

  renderer.outputColorSpace = THREE.LinearSRGBColorSpace

  const solidMat = new THREE.MeshStandardMaterial({ color: 0x888888, roughness: 0.6, metalness: 0.2 })
  const bladeMat = new THREE.MeshStandardMaterial({ color: 0x666666, roughness: 0.5, metalness: 0.4 })
  const ringMat  = new THREE.MeshPhongMaterial({ color: 0xbbbbbb, flatShading: false })

  const sceneRoot = new THREE.Group()
  scene.add(sceneRoot)

  const halos = new THREE.Group()
  halos.position.z = -6
  halos.scale.set(0, 0, 0)
  sceneRoot.add(halos)

  const halo1 = new THREE.Mesh(new THREE.TorusGeometry(12, 0.3, 4, 64), ringMat)
  halos.add(halo1)
  const halo2 = new THREE.Mesh(new THREE.TorusGeometry(15, 0.5, 4, 64), ringMat)
  halo2.rotation.x = Math.PI / 2
  halos.add(halo2)

  const monolithGroup = new THREE.Group()
  sceneRoot.add(monolithGroup)

  const halfGeo = new THREE.BoxGeometry(3.5, 18, 4)
  halfGeo.translate(1.75, 0, 0) 
  halfGeo.computeVertexNormals()
  
  const monoLeft = new THREE.Mesh(halfGeo, solidMat)
  monoLeft.scale.x = -1 
  const monoRight = new THREE.Mesh(halfGeo, solidMat)
  
  monolithGroup.add(monoLeft)
  monolithGroup.add(monoRight)

  interface WingData {
    group: THREE.Group
    feathers: THREE.Mesh[]
  }

  function createExaggeratedCyberWing(isLeft: boolean): WingData {
    const group = new THREE.Group()
    const dir = isLeft ? -1 : 1
    const feathers: THREE.Mesh[] = []
    const numBlades = 14

    const bladeGeo = new THREE.OctahedronGeometry(1, 4)
    bladeGeo.scale(1, 10, 0.6)
    bladeGeo.translate(0, 5, 0)
    bladeGeo.computeVertexNormals()

    for (let i = 0; i < numBlades; i++) {
      const t = i / (numBlades - 1)
      const blade = new THREE.Mesh(bladeGeo, bladeMat)
      const length = 1.8 - t * 1.0
      const width = 1.2 - t * 0.5
      const posX = dir * (2.5 + t * 6)
      const posY = -3.0 + t * 9
      const posZ = -t * 6
      const rotZ = dir * (-1.2 - t * 1.0)
      const rotY = dir * (t * 0.6)
      const rotX = t * 0.4

      blade.userData = {
        tPos: new THREE.Vector3(posX, posY, posZ),
        tRot: new THREE.Euler(rotX, rotY, rotZ),
        tScale: new THREE.Vector3(width, length, 1.0),
      }

      blade.scale.set(0.1, 0.1, 0.1)
      blade.position.set(dir * 2.0, -3.0, 0)
      blade.rotation.set(0, 0, dir * -1.5)

      feathers.push(blade)
      group.add(blade)
    }
    group.position.z = -1
    return { group, feathers }
  }

  const wingL = createExaggeratedCyberWing(true)
  const wingR = createExaggeratedCyberWing(false)
  wingL.group.visible = false
  wingR.group.visible = false
  sceneRoot.add(wingL.group)
  sceneRoot.add(wingR.group)

  const dc = createDitheredComposer(renderer, scene, camera, W, H)
  dc.setExposure(1.8)

  const overlay = document.createElement('div')
  overlay.style.position = 'absolute'
  overlay.style.inset = '0'
  overlay.style.display = 'flex'
  overlay.style.alignItems = 'center'
  overlay.style.justifyContent = 'center'
  overlay.style.pointerEvents = 'none'
  overlay.style.opacity = '0'
  overlay.style.transition = 'opacity 0.1s'
  overlay.style.zIndex = '20'

  const revText = document.createElement('div')
  revText.innerText = 'CONTRACT SEALED'
  revText.style.background = '#000'
  revText.style.color = '#fff'
  revText.style.fontFamily = 'var(--font-title), serif'
  revText.style.fontSize = 'clamp(32px, 6vw, 72px)'
  revText.style.fontWeight = '900'
  revText.style.padding = '16px 40px'
  revText.style.border = '4px solid #fff'
  revText.style.boxShadow = '12px 12px 0 #000'
  revText.style.letterSpacing = '0.08em'
  revText.style.textTransform = 'uppercase'
  overlay.appendChild(revText)
  mount.appendChild(overlay)

  let animState: AnimState = 'idle'
  let phaseStart = 0
  let baseTime = performance.now()
  let rafId = 0
  let completionCallback: (() => void) | null = null
  let monolithRotAtEruption = 0

  const bladeFoldStart: THREE.Euler[] = []

  const easeOutExpo = (t: number) => t >= 1 ? 1 : 1 - Math.pow(2, -10 * t)
  const easeOutBack = (t: number) => {
    const c1 = 1.70158
    return 1 + (c1 + 1) * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2)
  }
  const easeInOutCubic = (t: number) => t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
  const clamp01 = (v: number) => Math.max(0, Math.min(1, v))
  const lerp = (a: number, b: number, t: number) => a + (b - a) * t

  function rotateHalos() {
    halo1.rotation.z -= 0.008
    halo1.rotation.x += 0.004
    halo2.rotation.y += 0.012
    halo2.rotation.z += 0.006
  }

  function tick() {
    rafId = requestAnimationFrame(tick)
    const now = performance.now()
    const t = (now - baseTime) * 0.001

    let shakeIntensity = 0

    if (animState === 'idle') {
      monolithGroup.position.y = Math.sin(t) * 0.4
      monolithGroup.rotation.y = t * 0.15
      camera.position.y = baseCamPos.y + Math.sin(t * 0.5) * 0.3
      currentLookTarget.lerp(baseLookTarget, 0.1)

    } else if (animState === 'inscription') {
      const et = clamp01((now - phaseStart) / 1000) 
      camera.position.z = lerp(baseCamPos.z, 20, easeOutExpo(et))
      camera.position.x = lerp(baseCamPos.x, 0, easeOutExpo(et))
      currentLookTarget.lerp(new THREE.Vector3(0, 0, 0), 0.1)
      monolithGroup.rotation.y = lerp(monolithRotAtEruption, 0, easeOutExpo(et))
      shakeIntensity = 0.05 * et
      if (et >= 1) {
        animState = 'erupting'
        phaseStart = now
        wingL.group.visible = true
        wingR.group.visible = true
      }

    } else if (animState === 'erupting') {
      const et = clamp01((now - phaseStart) / 1500) 
      shakeIntensity = (1 - et) * 0.5 

      for (let i = 0; i < wingL.feathers.length; i++) {
        const bl = wingL.feathers[i]
        const br = wingR.feathers[i]
        const delay = (i % 14) * 0.03
        const localT = clamp01((et - delay) / 0.5)

        bl.scale.lerpVectors(new THREE.Vector3(0.1,0.1,0.1), bl.userData.tScale, easeOutBack(localT))
        br.scale.copy(bl.scale)
        
        bl.position.lerpVectors(new THREE.Vector3(-2,-3,0), bl.userData.tPos, easeOutExpo(localT))
        br.position.copy(bl.position).setX(-bl.position.x)

        bl.rotation.set(
          lerp(0, bl.userData.tRot.x, easeOutExpo(localT)),
          lerp(0, bl.userData.tRot.y, easeOutExpo(localT)),
          lerp(1.5, bl.userData.tRot.z, easeOutExpo(localT))
        )
        br.rotation.set(bl.rotation.x, -bl.rotation.y, -bl.rotation.z)
      }

      halos.scale.setScalar(easeOutExpo(et))
      rotateHalos()

      if (et >= 1) { 
        animState = 'breathing'
        phaseStart = now 
      }

    } else if (animState === 'breathing') {
      const breathT = (now - phaseStart) * 0.003
      wingL.feathers.forEach((b, i) => b.rotation.x = b.userData.tRot.x + Math.sin(breathT + i*0.2)*0.08)
      wingR.feathers.forEach((b, i) => b.rotation.x = b.userData.tRot.x + Math.sin(breathT + i*0.2)*0.08)
      rotateHalos()
      
      if (now - phaseStart >= 1500) { 
        animState = 'revelation'
        phaseStart = now 
      }

    } else if (animState === 'revelation') {
      const rt = clamp01((now - phaseStart) / 800)
      const flash = Math.sin(rt * Math.PI)
      dc.setExposure(1.8 + flash * 4.0) 
      
      if (rt > 0.1 && rt < 0.9) overlay.style.opacity = '1'
      else overlay.style.opacity = '0'

      rotateHalos()
      if (rt >= 1) {
        overlay.style.opacity = '0'
        dc.setExposure(1.8)
        
        bladeFoldStart.length = 0
        ;[...wingL.feathers, ...wingR.feathers].forEach(b => 
          bladeFoldStart.push(new THREE.Euler(b.rotation.x, b.rotation.y, b.rotation.z))
        )
        
        animState = 'ingress'
        phaseStart = now
      }

    } else if (animState === 'ingress') {
      // ── Ascension: the whole angel rises into blooming light ───────────────
      // No monolith split, no camera fly-through. The figure lifts as the wings
      // sweep upward to the heavens and the dither blooms to pure white in place,
      // so the outro stays centered and ends on a clean full-white screen.
      const it = clamp01((now - phaseStart) / 3000)
      const e = easeInOutCubic(it)

      // Whole composition rises and recedes slightly
      sceneRoot.position.y = lerp(0, 11, e)
      sceneRoot.position.z = lerp(0, -7, e)

      // Camera tilts up to follow, with a gentle push-in (no dive)
      currentLookTarget.y = lerp(0, 9, e)
      camera.position.z = lerp(20, 17, e)

      // Wings sweep UP into a tall upward fan — tips point up-and-out, blades
      // lift and gather inward (less horizontal spread than the eruption), with
      // outer blades reaching higher and wider. Like an angel raising its wings
      // to the heavens. We lerp both position and rotation toward an explicit
      // raised target so the wings open upward instead of curling into a cradle.
      const allBlades = [...wingL.feathers, ...wingR.feathers]
      allBlades.forEach((blade, i) => {
        const start = bladeFoldStart[i]
        if (!start) return
        const dir = i < 14 ? -1 : 1
        const frac = (i % 14) / 13
        const tpos = blade.userData.tPos as THREE.Vector3

        // Raised target pose — a FULLY EXPANDED wingspan.
        // Bases stay gathered near the shoulder; the long blades fan out via
        // rotation, radiating up-and-out into a wide arc (peacock-tail / spread
        // eagle). Inner blades stand vertical, outer blades sweep far outward.
        const px = dir * (1.5 + frac * 2.0)   // bases near shoulder, slight spread
        const py = frac * 1.0
        const pz = -1
        const rx = 0
        const ry = dir * frac * 0.2           // slight 3D fan
        const rz = -dir * (0.05 + frac * 1.2) // inner vertical → outer ~70° out

        blade.position.set(
          lerp(tpos.x, px, e),
          lerp(tpos.y, py, e),
          lerp(tpos.z, pz, e),
        )
        blade.rotation.set(
          lerp(start.x, rx, e) + Math.sin(it * Math.PI * 3 + i * 0.25) * 0.05 * (1 - e),
          lerp(start.y, ry, e),
          lerp(start.z, rz, e),
        )
      })

      // Halo expands and brightens behind the ascending figure
      halos.scale.setScalar(lerp(1, 4.5, e))

      // Bloom to white over the final 55% — the figure is consumed by light
      const bloom = clamp01((it - 0.45) / 0.55)
      dc.setExposure(lerp(1.8, 7.0, bloom))

      rotateHalos()

      if (it >= 1) {
        animState = 'fading'
        phaseStart = now
        // Scene is already bloomed to near-white; fade the canvas straight to
        // transparent onto the opaque white solid-bg (uniform dissolve, no scale).
        // App.tsx bridges the resulting white into the app → white → white → app.
        mount.style.transition = 'opacity 500ms ease'
        mount.style.opacity = '0'
      }

    } else if (animState === 'fading') {
      const ft = clamp01((now - phaseStart) / 500)
      // Already white — just keep the halo drifting and hold for the CSS fade.
      rotateHalos()
      if (ft >= 1) {
        animState = 'done'
        completionCallback?.()
      }
    } else {
      rotateHalos()
    }

    const shakeX = shakeIntensity > 0 ? (Math.random() - 0.5) * shakeIntensity * 4 : 0
    const shakeY = shakeIntensity > 0 ? (Math.random() - 0.5) * shakeIntensity * 4 : 0

    if (shakeIntensity > 0) {
      camera.position.x += shakeX
      camera.position.y += shakeY
    }

    camera.lookAt(currentLookTarget)
    dc.composer.render()

    if (shakeIntensity > 0) {
      camera.position.x -= shakeX
      camera.position.y -= shakeY
    }
  }

  tick()

  function onResize() {
    const nW = window.innerWidth
    const nH = window.innerHeight
    renderer.setSize(nW, nH)
    dc.setSize(nW, nH)
    camera.aspect = nW / nH
    camera.updateProjectionMatrix()
  }
  window.addEventListener('resize', onResize)

  return {
    triggerEruption(onComplete: () => void) {
      if (animState !== 'idle') return
      completionCallback = onComplete
      monolithRotAtEruption = monolithGroup.rotation.y
      animState = 'inscription'
      phaseStart = performance.now()
    },
    dispose() {
      cancelAnimationFrame(rafId)
      window.removeEventListener('resize', onResize)
      if (mount.contains(renderer.domElement)) mount.removeChild(renderer.domElement)
      if (mount.contains(overlay)) mount.removeChild(overlay)
      renderer.dispose()
    },
  }
}
