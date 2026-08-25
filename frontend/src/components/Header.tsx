import { useApp } from '../context/AppContext'
import { BRAND_LOGO_SRC } from '../lib/brand'

interface Props {
  mobileDrawer?: 'circles' | 'info' | null
  circlesOpen?: boolean
  infoOpen?: boolean
  onToggleCircles?: () => void
  onToggleInfo?: () => void
}

function PanelGlyph({ side, open }: { side: 'left' | 'right'; open: boolean }) {
  return (
    <span className={`sidebar-toggle-glyph sidebar-toggle-glyph--${side}${open ? ' is-open' : ''}`} aria-hidden="true">
      <span className="sidebar-toggle-glyph__divider" />
    </span>
  )
}

export default function Header({ circlesOpen, infoOpen, onToggleCircles, onToggleInfo }: Props) {
  const { circles, activeCircleId } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

  return (
    <header className="app-header sys-window z-[100] flex items-center justify-between gap-4 px-5 min-h-[48px] font-mono text-[11px] uppercase font-bold">
      <div className="flex items-center gap-3 min-w-0">
        <img className="brand-mark shrink-0" src={BRAND_LOGO_SRC} alt="Enoxian" />
        {onToggleCircles && (
          <button
            onClick={onToggleCircles}
            className={`mobile-header-btn mobile-header-btn--circles${circlesOpen ? ' active' : ''}`}
            aria-label={circlesOpen ? 'Close circles sidebar' : 'Open circles sidebar'}
            aria-expanded={circlesOpen}
            title={circlesOpen ? 'Close circles sidebar' : 'Open circles sidebar'}
          >
            <PanelGlyph side="left" open={!!circlesOpen} />
          </button>
        )}
        {activeCircle && (
          <div className="header-circle-identity">
            <span className="font-bold tracking-widest truncate">{activeCircle.circle_name}</span>
            <span className={`header-circle-state header-circle-state--${activeCircle.disabled ? 'void' : 'live'}`}>
              {activeCircle.disabled ? 'OFF' : 'LIVE'}
            </span>
          </div>
        )}
      </div>

      <div className="header-actions flex items-center gap-3 shrink-0 min-w-0">
        {onToggleInfo && (
          <button
            onClick={onToggleInfo}
            className={`mobile-header-btn mobile-header-btn--info${infoOpen ? ' active' : ''}`}
            aria-label={infoOpen ? 'Close workspace sidebar' : 'Open workspace sidebar'}
            aria-expanded={infoOpen}
            title={infoOpen ? 'Close workspace sidebar' : 'Open workspace sidebar'}
          >
            <PanelGlyph side="right" open={!!infoOpen} />
          </button>
        )}
      </div>
    </header>
  )
}
