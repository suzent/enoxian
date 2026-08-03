import { useState } from 'react'
import { useApp } from '../context/AppContext'
import { BRAND_LOGO_SRC } from '../lib/brand'
import { Menu, PanelRight, Settings } from 'lucide-react'
import DeviceSettings from './DeviceSettings'

interface Props {
  mobileDrawer?: 'circles' | 'info' | null
  infoOpen?: boolean
  onToggleCircles?: () => void
  onToggleInfo?: () => void
}

export default function Header({ mobileDrawer, infoOpen, onToggleCircles, onToggleInfo }: Props) {
  const { status, circles, activeCircleId } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)
  const [settingsOpen, setSettingsOpen] = useState(false)

  return (
    <header className="app-header sys-window z-[100] flex items-center justify-between gap-4 px-5 min-h-[48px] font-mono text-[11px] uppercase font-bold">
      <div className="flex items-center gap-3 min-w-0">
        <img className="brand-mark shrink-0" src={BRAND_LOGO_SRC} alt="Enoxian" />
        {/* Mobile-only circles toggle */}
        {onToggleCircles && (
          <button
            onClick={onToggleCircles}
            className={`mobile-header-btn mobile-header-btn--circles${mobileDrawer === 'circles' ? ' active' : ''}`}
            aria-label="Open circles"
          >
            <Menu size={18} strokeWidth={2.5} />
          </button>
        )}
        {activeCircle && (
          <span className="font-bold tracking-widest truncate">{activeCircle.circle_name}</span>
        )}
      </div>

      <div className="header-actions flex items-center gap-3 shrink-0 min-w-0">
        <div className="header-status flex items-center gap-3 text-slate font-normal min-w-0">
          {status && (
            <>
              <button
                className="sys-badge header-agent-id header-settings-btn flex items-center gap-1.5"
                onClick={() => setSettingsOpen(true)}
                title="Device settings — agent mention reactions"
                aria-label="Open device settings"
              >
                <Settings size={12} strokeWidth={2.5} aria-hidden="true" />
                {status.agent_id}
              </button>
              <span className="sys-badge header-docs-count">docs {status.docs}</span>
            </>
          )}
        </div>
        {/* Mobile-only info toggle */}
        {onToggleInfo && (
          <button
            onClick={onToggleInfo}
            className={`mobile-header-btn mobile-header-btn--info${infoOpen ? ' active' : ''}`}
            aria-label="Open circle info"
          >
            <PanelRight size={18} strokeWidth={2.5} />
          </button>
        )}
      </div>
      {settingsOpen && <DeviceSettings onClose={() => setSettingsOpen(false)} />}
    </header>
  )
}
