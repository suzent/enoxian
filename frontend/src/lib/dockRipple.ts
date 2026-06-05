import * as THREE from 'three'

export function createDockRipple(pose: { x: number; y: number }, scene: THREE.Scene) {
  const group = new THREE.Group()
  // Spawn slightly in front of the dock to avoid depth clipping
  group.position.set(pose.x, pose.y, 0.05)
  
  // 1. A filled center that expands and fades to "fill" the dock
  const fillGeo = new THREE.CircleGeometry(1, 32)
  const fillMat = new THREE.MeshBasicMaterial({ 
    color: 0x000000, 
    transparent: true, 
    opacity: 0.6, // Increased visibility
    depthTest: false, // Force render over everything
    depthWrite: false
  })
  const fillMesh = new THREE.Mesh(fillGeo, fillMat)
  
  // 2. An expanding outline ring for the ripple edge
  const ringGeo = new THREE.RingGeometry(0.85, 1.0, 32)
  const ringMat = new THREE.MeshBasicMaterial({
    color: 0x000000,
    transparent: true,
    opacity: 1.0, // Increased visibility
    depthTest: false, // Force render over everything
    depthWrite: false
  })
  const ringMesh = new THREE.Mesh(ringGeo, ringMat)

  group.add(fillMesh)
  group.add(ringMesh)
  
  // Render order to force it on top of the UI
  group.renderOrder = 999
  fillMesh.renderOrder = 999
  ringMesh.renderOrder = 999

  scene.add(group)

  const startTime = performance.now()
  const duration = 600 // Snappy 600ms fade and expand

  // Target radius of 0.05 exactly bounds the 36x36px dock area on screen
  const targetRadius = 0.05 

  const animateRipple = () => {
    const elapsed = performance.now() - startTime
    const p = Math.min(elapsed / duration, 1)

    if (p >= 1) {
      scene.remove(group)
      fillGeo.dispose()
      fillMat.dispose()
      ringGeo.dispose()
      ringMat.dispose()
      return
    }

    // Smooth easing for expansion (easeOutCubic)
    const easeOut = 1 - Math.pow(1 - p, 3)
    const currentScale = Math.max(targetRadius * easeOut, 0.001)
    group.scale.setScalar(currentScale)

    // Smooth linear fade out to "smooth out" the effect
    fillMat.opacity = 0.3 * (1 - p)
    ringMat.opacity = 0.8 * (1 - p)

    requestAnimationFrame(animateRipple)
  }

  requestAnimationFrame(animateRipple)
}