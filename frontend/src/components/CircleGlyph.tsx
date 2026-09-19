import { useEffect, useRef, useState } from 'react'
import { circleFamily, drawCircleMark, type CircleFamily } from '../lib/circleIdentity'

interface Props {
  name: string
  circleId?: string
  size: number
  family?: CircleFamily
  voided?: boolean
  active?: boolean
  unread?: boolean
  engaged?: boolean
}

export default function CircleGlyph({ name, circleId = name, size, family = circleFamily(circleId), voided, active, unread, engaged }: Props) {
  const ref = useRef<HTMLCanvasElement>(null)
  const arrival = useRef(0)
  const [hovered, setHovered] = useState(false)
  const pointer = useRef({ x: 0, y: 0 })
  const motion = useRef({ x: 0, y: 0, hover: 0 })
  useEffect(() => { if (unread) arrival.current = performance.now() }, [unread, circleId])

  useEffect(() => {
    const canvas = ref.current
    const ctx = canvas?.getContext('2d')
    if (!canvas || !ctx) return
    const media = matchMedia('(prefers-reduced-motion: reduce)')
    let raf = 0, last = 0
    const draw = (now: number) => {
      if (now - last < 32) { raf = requestAnimationFrame(draw); return }
      const delta = Math.min((now - (last || now - 32)) / 1000, .1)
      last = now
      const dpr = Math.min(devicePixelRatio || 1, 2)
      if (canvas.width !== size * dpr) { canvas.width = size * dpr; canvas.height = size * dpr }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, size, size)
      const color = getComputedStyle(canvas).color
      const t = media.matches ? 0 : now / 1000, r = size / 3.1
      const target = !voided && !media.matches && (hovered || engaged) ? 1 : 0
      const blend = 1 - Math.exp(-delta * 12)
      const pose = motion.current
      pose.hover += (target - pose.hover) * blend
      pose.x += (pointer.current.x * target - pose.x) * blend
      pose.y += (pointer.current.y * target - pose.y) * blend
      if (media.matches) { pose.hover = 0; pose.x = 0; pose.y = 0 }
      const turn = (active && !voided ? t * .24 : 0) + pose.hover * Math.sin(t * 2.4) * .5 + pose.x * .3
      drawCircleMark(ctx, size / 2 + pose.x * r * .2, size / 2 + pose.y * r * .2,
        r * (1 + pose.hover * .22), circleId, family, color, 1, turn)
      if (unread && !voided) {
        const breath = media.matches ? .8 : .5 + .5 * Math.sin(t * 1.65 - 1)
        ctx.fillStyle = color
        ctx.strokeStyle = color
        for (let i = 0; i < 64; i++) {
          const a = i * Math.PI / 32
          ctx.globalAlpha = (i % 3 ? .2 : .5) + breath * .2
          ctx.fillRect(size / 2 + Math.cos(a) * r * (1.12 + breath * .025), size / 2 + Math.sin(a) * r * (1.12 + breath * .025), 1, 1)
        }
        ctx.globalAlpha = .55 + breath * .45
        const x = size / 2 + r * .82, y = size / 2 - r * .82, length = 2 + breath * 1.5
        ctx.beginPath()
        ctx.moveTo(x - length, y); ctx.lineTo(x + length, y)
        ctx.moveTo(x, y - length); ctx.lineTo(x, y + length)
        ctx.stroke()
        const elapsed = (now - arrival.current) / 1000
        if (elapsed < 1.4 && !media.matches) {
          ctx.globalAlpha = (1 - elapsed / 1.4) * .6
          ctx.beginPath()
          ctx.arc(size / 2, size / 2, r * (.93 + elapsed * .32), 0, Math.PI * 2)
          ctx.stroke()
        }
      }
      ctx.globalAlpha = 1
      // Idle and reduced-motion marks render once, without a perpetual loop.
      if (!media.matches && (active || unread || hovered || engaged || pose.hover > .001) && !voided) raf = requestAnimationFrame(draw)
    }
    const refresh = () => { cancelAnimationFrame(raf); last = 0; raf = requestAnimationFrame(draw) }
    media.addEventListener('change', refresh)
    const observer = new MutationObserver(refresh)
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme'] })
    refresh()
    return () => { cancelAnimationFrame(raf); media.removeEventListener('change', refresh); observer.disconnect() }
  }, [circleId, size, family, voided, active, unread, engaged, hovered])

  return <canvas ref={ref}
    onPointerEnter={() => setHovered(true)}
    onPointerMove={event => {
      const rect = event.currentTarget.getBoundingClientRect()
      pointer.current = { x: (event.clientX - rect.left) / rect.width * 2 - 1, y: (event.clientY - rect.top) / rect.height * 2 - 1 }
    }}
    onPointerLeave={() => { setHovered(false); pointer.current = { x: 0, y: 0 } }}
    style={{ width: size, height: size, display: 'block', opacity: voided ? .25 : 1 }} aria-hidden="true" />
}
