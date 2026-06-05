const PARTICLE_COUNT = 48
const DURATION_MS    = 800
const GRAVITY        = 0.28
const BOUNCE_DAMP    = 0.5

export function triggerDockBurst() {
  const dockEl  = document.querySelector('[data-circle-dock]') as HTMLElement | null
  const glyphEl = dockEl?.querySelector('canvas') as HTMLElement | null
  const anchor  = glyphEl ?? dockEl
  if (!dockEl || !anchor) return

  const dockRect  = dockEl.getBoundingClientRect()
  const glyphRect = anchor.getBoundingClientRect()
  const cx = glyphRect.left - dockRect.left + glyphRect.width  / 2
  const cy = glyphRect.top  - dockRect.top  + glyphRect.height

  const canvas = document.createElement('canvas')
  canvas.width  = dockRect.width
  canvas.height = dockRect.height
  canvas.style.cssText = 'position:absolute;inset:0;pointer-events:none;z-index:20;'
  dockEl.style.position = 'relative'
  dockEl.appendChild(canvas)

  const ctx = canvas.getContext('2d')!

  type Particle = { x: number; y: number; vx: number; vy: number; r: number; floor: number }
  const particles: Particle[] = []
  for (let i = 0; i < PARTICLE_COUNT; i++) {
    const angle = -Math.PI * (0.05 + Math.random() * 0.9)
    const speed = 3.5 + Math.random() * 5.5
    const r     = 1.8 + Math.random() * 2.8
    const floor = cy + 2 + Math.random() * 6
    particles.push({
      x: cx + (Math.random() - 0.5) * 10, y: cy,
      vx: Math.cos(angle) * speed, vy: Math.sin(angle) * speed,
      r, floor,
    })
  }

  const start = performance.now()
  function draw() {
    const t = (performance.now() - start) / DURATION_MS
    if (t >= 1) { canvas.remove(); return }
    ctx.clearRect(0, 0, canvas.width, canvas.height)
    const opacity = t < 0.25 ? 1 : Math.pow(1 - (t - 0.25) / 0.75, 1.4)
    for (const p of particles) {
      p.vy += GRAVITY; p.x += p.vx; p.y += p.vy
      if (p.vy > 0 && p.y >= p.floor) { p.y = p.floor; p.vy = -p.vy * BOUNCE_DAMP; p.vx *= 0.7 }
      const pOpacity = opacity * (0.6 + (p.r / 4.6) * 0.4)
      ctx.fillStyle = `rgba(17,17,17,${pOpacity.toFixed(3)})`
      ctx.beginPath(); ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2); ctx.fill()
    }
    requestAnimationFrame(draw)
  }
  requestAnimationFrame(draw)
}
