import { useApp } from '../context/AppContext'
import { BRAND_LOGO_SRC } from '../lib/brand'

interface Props {
  mobileDrawer?: 'circles' | 'info' | null
  onToggleCircles?: () => void
  onToggleInfo?: () => void
}

export default function Header({ mobileDrawer, onToggleCircles, onToggleInfo }: Props) {
  const { status, circles, activeCircleId } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

  return (
    <header className="app-header sys-window z-[100] flex items-center justify-between gap-4 px-5 min-h-[48px] font-mono text-[11px] uppercase font-bold">
      <div className="flex items-center gap-3 min-w-0">
        <img className="brand-mark shrink-0" src={BRAND_LOGO_SRC} alt="Enoxian" />
        {/* Mobile-only circles toggle */}
        {onToggleCircles && (
          <button
            onClick={onToggleCircles}
            className={`mobile-header-btn${mobileDrawer === 'circles' ? ' active' : ''}`}
          >
            ☰
          </button>
        )}
        {activeCircle && (
          <span className="font-bold tracking-widest truncate">{activeCircle.circle_name}</span>
        )}
      </div>

      <div className="flex items-center gap-3 shrink-0">
        <div className="flex items-center gap-3 text-slate font-normal">
          {status && (
            <>
              <span className="sys-badge">{status.agent_id}</span>
              <span className="sys-badge">docs {status.docs}</span>
            </>
          )}
        </div>
        {/* Mobile-only info toggle */}
        {onToggleInfo && (
          <button
            onClick={onToggleInfo}
            className={`mobile-header-btn${mobileDrawer === 'info' ? ' active' : ''}`}
          >
            ⊞
          </button>
        )}
      </div>
    </header>
  )
}
