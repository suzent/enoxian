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

  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false })
  renderer.setSize(W, H)
  // Use devicePixelRatio to fix diagonal aliasing/staircase jaggedness when rotating
  renderer.setPixelRatio(window.devicePixelRatio)
  renderer.setClearColor(0xffffff, 1)
  mount.appendChild(renderer.domElement)

  const scene = new THREE.Scene()
  scene.background = new THREE.Color(0xf0f0f0) // 将死白改为极浅的冷灰色，增加空间感
  
  const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 150)
  const baseCamPos = new THREE.Vector3(0, -4, 34)
  camera.position.copy(baseCamPos)
  
  const baseLookTarget = new THREE.Vector3(0, 0, 0)
  const currentLookTarget = new THREE.Vector3(0, 0, 0)
  camera.lookAt(baseLookTarget)

  // 降低全局环境光，增加阴影对比
  scene.add(new THREE.AmbientLight(0xffffff, 0.2))
  
  // 主光源 (Key Light)：从左上方打过来，拉出高光和强烈的阴影边缘
  const keyLight = new THREE.DirectionalLight(0xffffff, 4.0)
  keyLight.position.set(-15, 20, 15)
  scene.add(keyLight)
  
  // 边缘光 (Rim Light)：从右后方打过来，照亮死黑的背光面，勾勒出石碑和翅膀的右侧轮廓线
  const rimLight = new THREE.DirectionalLight(0xffffff, 2.0)
  rimLight.position.set(15, -5, -15)
  scene.add(rimLight)
  
  // 补光 (Fill Light)：微弱的正面底光，避免正面出现纯黑死角
  const fillLight = new THREE.DirectionalLight(0xffffff, 0.4)
  fillLight.position.set(0, 5, 5)
  scene.add(fillLight)

  renderer.outputColorSpace = THREE.LinearSRGBColorSpace

  function createStoneTexture() {
    const canvas = document.createElement('canvas')
    canvas.width = 512
    canvas.height = 512
    const ctx = canvas.getContext('2d')!
    
    // 基础岩石灰
    ctx.fillStyle = '#888888'
    ctx.fillRect(0, 0, canvas.width, canvas.height)
    
    // 1. 叠加大型柔和的明暗色块，模拟大理石/岩石的光影起伏
    for (let i = 0; i < 400; i++) {
      ctx.fillStyle = Math.random() > 0.5 ? 'rgba(255,255,255,0.04)' : 'rgba(0,0,0,0.04)'
      ctx.beginPath()
      const x = Math.random() * canvas.width
      const y = Math.random() * canvas.height
      const r = Math.random() * 80 + 20
      ctx.arc(x, y, r, 0, Math.PI * 2)
      ctx.fill()
    }
    
    // 2. 叠加细微的高频噪点颗粒，模拟石材表面的粗糙颗粒感
    for (let i = 0; i < 150000; i++) {
      const x = Math.random() * canvas.width
      const y = Math.random() * canvas.height
      ctx.fillStyle = Math.random() > 0.5 ? 'rgba(255,255,255,0.06)' : 'rgba(0,0,0,0.06)'
      ctx.fillRect(x, y, 1, 1)
    }

    const tex = new THREE.CanvasTexture(canvas)
    tex.anisotropy = renderer.capabilities.getMaxAnisotropy()
    tex.minFilter = THREE.LinearMipmapLinearFilter
    tex.magFilter = THREE.LinearFilter
    tex.wrapS = THREE.RepeatWrapping
    tex.wrapT = THREE.RepeatWrapping
    return tex
  }

  const monoTex = createStoneTexture()
  const solidMat = new THREE.MeshStandardMaterial({ 
    color: 0xcccccc, // 基础颜色
    map: monoTex,
    bumpMap: monoTex,
    bumpScale: 0.05, // 极其轻微的凹凸感，让 Dither 产生自然的明暗渐变散点
    roughness: 0.95, // 石头的高粗糙度
    metalness: 0.0,
    emissive: 0x222222 // 增加一点自发光，确保暗面不会变成死黑，从而保留石头纹理的细节
  })
  
  // 既然有了大面积留白的纹理，线框就不需要了，以免画面过于杂乱
  // 移除 wireframeMat

  // 生成羽毛材质专属的细丝纹理 (Feather Barbs)
  function createFeatherTexture() {
    const canvas = document.createElement('canvas')
    canvas.width = 256
    canvas.height = 1024
    const ctx = canvas.getContext('2d')!
    
    // 纯白底色
    ctx.fillStyle = '#ffffff'
    ctx.fillRect(0, 0, canvas.width, canvas.height)
    
    // 中轴线 (羽轴 Rachis)
    ctx.fillStyle = '#222222'
    ctx.fillRect(canvas.width / 2 - 4, 0, 8, canvas.height)
    
    // 绘制密集的斜向细丝 (羽支 Barbs)
    ctx.strokeStyle = '#666666'
    ctx.lineWidth = 1
    
    for (let y = 0; y < canvas.height; y += 4) {
      // 左侧羽支 (向左下倾斜)
      ctx.beginPath()
      ctx.moveTo(canvas.width / 2, y)
      // 添加一些随机的长度和弯曲度，模拟真实羽毛的柔软感
      const leftLen = canvas.width / 2 - 10 + Math.random() * 10
      ctx.quadraticCurveTo(canvas.width / 4, y + 20, canvas.width / 2 - leftLen, y + 40)
      ctx.stroke()
      
      // 右侧羽支 (向右下倾斜)
      ctx.beginPath()
      ctx.moveTo(canvas.width / 2, y)
      const rightLen = canvas.width / 2 - 10 + Math.random() * 10
      ctx.quadraticCurveTo(canvas.width * 0.75, y + 20, canvas.width / 2 + rightLen, y + 40)
      ctx.stroke()
    }
    
    // 添加一些柔和的垂直噪点，模拟羽绒的质感
    for (let i = 0; i < 20000; i++) {
      const x = Math.random() * canvas.width
      const y = Math.random() * canvas.height
      ctx.fillStyle = 'rgba(0,0,0,0.05)'
      ctx.fillRect(x, y, 2, 4)
    }

    const tex = new THREE.CanvasTexture(canvas)
    tex.anisotropy = renderer.capabilities.getMaxAnisotropy()
    tex.minFilter = THREE.LinearMipmapLinearFilter
    tex.magFilter = THREE.LinearFilter
    tex.wrapS = THREE.RepeatWrapping
    tex.wrapT = THREE.RepeatWrapping
    return tex
  }

  const featherTex = createFeatherTexture()

  // 羽毛材质：使用带有羽支纹理的漫反射材质，模拟真实鸟类羽毛的柔和质感
  const bladeMat = new THREE.MeshStandardMaterial({ 
    color: 0xffffff, // 纯白羽毛
    map: featherTex,
    roughness: 1.0,  // 羽毛表面是完全漫反射的，没有高光
    metalness: 0.0,  
    flatShading: false, // 恢复平滑着色，避免出现硬朗的几何切面
    alphaTest: 0.5 // (可选) 如果未来改成片面羽毛，这里留个 alpha 阈值
  })
  
  // 改为深色半透明的双面全息材质：使用 NormalBlending 和较深的颜色，确保在白色背景中清晰可见
  // 双面渲染 (DoubleSide) 和无深度写入 (depthWrite: false) 能制造出完美的幽灵能量体叠加感，同时解决穿模切割
  const ringMat = new THREE.MeshPhongMaterial({ 
    color: 0x222222, 
    emissive: 0x111111,
    specular: 0xaaaaaa,
    shininess: 30,
    transparent: true,
    opacity: 0.6,
    depthWrite: false,
    side: THREE.DoubleSide
  })

  const sceneRoot = new THREE.Group()
  scene.add(sceneRoot)

  const halos = new THREE.Group()
  halos.position.z = -6
  halos.scale.set(0, 0, 0)
  halos.visible = false // Hide completely until erupting
  sceneRoot.add(halos)

  const halo1 = new THREE.Mesh(new THREE.TorusGeometry(12, 0.3, 4, 64), ringMat)
  halos.add(halo1)
  const halo2 = new THREE.Mesh(new THREE.TorusGeometry(15, 0.5, 4, 64), ringMat)
  halo2.rotation.x = Math.PI / 2
  halos.add(halo2)

  const monolithGroup = new THREE.Group()
  sceneRoot.add(monolithGroup)

  // 增加分段数以产生有趣的几何线框结构
  const halfGeo = new THREE.BoxGeometry(3.5, 18, 4, 3, 12, 2)
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
  overlay.style.transition = 'opacity 0.2s ease, transform 0.2s cubic-bezier(0.2, 2.0, 0.4, 1)'
  overlay.style.transform = 'scale(1.15)'
  overlay.style.zIndex = '20'

  const revText = document.createElement('div')
  revText.innerText = 'ENOXIAN PROTOCOL ENGAGED'
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
        halos.visible = true // Reveal halos when eruption starts
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
      const rt = clamp01((now - phaseStart) / 2000)
      
      // Removed the blinding flash entirely per user request.
      // The exposure stays at 1.8 so the geometry details remain visible.
      dc.setExposure(1.8) 
      
      if (rt > 0.1 && rt < 0.9) overlay.style.opacity = '1'
      else overlay.style.opacity = '0'

      // Keep the wings breathing so the animation doesn't look "paused"
      const breathT = (now - baseTime) * 0.003
      wingL.feathers.forEach((b, i) => b.rotation.x = b.userData.tRot.x + Math.sin(breathT + i*0.2)*0.08)
      wingR.feathers.forEach((b, i) => b.rotation.x = b.userData.tRot.x + Math.sin(breathT + i*0.2)*0.08)

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

      // Keep exposure fixed at baseline so nothing fades to white / gets blown out
      // (The flash has already happened, now the figure just ascends gracefully)
      dc.setExposure(1.8)

      rotateHalos()

      if (it >= 1) {
        animState = 'fading'
        phaseStart = now
        // Scene fades to transparent via CSS while the figure is fully visible
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
