import { useEffect, useRef } from 'react'
import { circleFamily, drawCircleMark, hash } from '../lib/circleIdentity'

export type RitualMode = 'init' | 'enter'
interface Props {
  ritual: { mode: RitualMode; label?: string; circleId: string } | null
  onComplete: () => void
}
const ease = (value: number) => { const t = Math.max(0, Math.min(1, value)); return t * t * (3 - 2 * t) }

/** Assemble the very same identity that will remain in the Circle header. */
export default function RitualTransition({ ritual, onComplete }: Props) {
  const ref = useRef<HTMLCanvasElement>(null)
  const complete = useRef(onComplete)
  complete.current = onComplete
  useEffect(() => {
    if (!ritual) return
    const canvas = ref.current
    const ctx = canvas?.getContext('2d')
    const media = matchMedia('(prefers-reduced-motion: reduce)')
    if (!canvas || !ctx || media.matches) { complete.current(); return }
    let raf = 0, done = false
    const finish = () => { if (!done) { done = true; cancelAnimationFrame(raf); complete.current() } }
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') finish() }
    const reduce = () => { if (media.matches) finish() }
    window.addEventListener('keydown', escape)
    media.addEventListener('change', reduce)
    const start = performance.now(), family = circleFamily(ritual.circleId)
    const draw = (now: number) => {
      const progress = (now - start) / 2200
      if (progress >= 1) { finish(); return }
      const w = innerWidth, h = innerHeight, dpr = Math.min(devicePixelRatio || 1, 2)
      if (canvas.width !== w * dpr || canvas.height !== h * dpr) { canvas.width = w * dpr; canvas.height = h * dpr }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, w, h)
      const style = getComputedStyle(canvas), color = style.color
      const build = ease(progress / .42), lock = ease((progress - .36) / .26), land = ease((progress - .61) / .34)
      const dock = Array.from(document.querySelectorAll('[data-circle-dock] canvas')).filter(el => !el.parentElement?.parentElement?.closest('[aria-hidden="true"]')).map(el => el.getBoundingClientRect()).find(rect => rect.width > 0 && rect.height > 0)
      const x = w * .5 + ((dock ? dock.left + dock.width / 2 : w * .5) - w * .5) * land
      const y = h * .41 + ((dock ? dock.top + dock.height / 2 : h * .41) - h * .41) * land
      const radius = Math.min(80, w * .21), r = radius + ((dock ? dock.width / 3.1 : radius) - radius) * land
      ctx.globalAlpha = 1 - ease((progress - .57) / .38)
      ctx.fillStyle = style.getPropertyValue('--bg-alabaster').trim() || getComputedStyle(document.body).backgroundColor
      ctx.fillRect(0, 0, w, h)
      ctx.fillStyle = color
      for (let i = 0; i < 72; i++) {
        const seed = hash(ritual.circleId + i), a = (seed % 6283) / 1000
        const distance = (70 + seed % 120) * (1 - build) + (15 + seed % 45) * build
        ctx.globalAlpha = (1 - ease(progress / .5)) * .55
        ctx.fillRect(w * .5 + Math.cos(a) * distance, h * .41 + Math.sin(a) * distance, 1.5, 1.5)
      }
      ctx.globalAlpha = dock ? 1 : 1 - land
      drawCircleMark(ctx, x, y, r, ritual.circleId, family, color, build, (1 - lock) * .65)
      ctx.globalAlpha = (1 - land) * build; ctx.fillStyle = color; ctx.textAlign = 'center'
      ctx.font = '14px ui-monospace, monospace'
      ctx.fillText(ritual.label || 'Circle', w / 2, h * .41 + radius + 40, w - 48)
      ctx.globalAlpha = 1
      raf = requestAnimationFrame(draw)
    }
    raf = requestAnimationFrame(draw)
    return () => { done = true; cancelAnimationFrame(raf); window.removeEventListener('keydown', escape); media.removeEventListener('change', reduce) }
  }, [ritual])
  if (!ritual) return null
  return <div className="circle-entry" aria-label="Entering Circle">
    <canvas ref={ref} aria-hidden="true" />
    <button type="button" onClick={() => complete.current()}>Skip <span aria-hidden="true">Esc</span></button>
  </div>
}
