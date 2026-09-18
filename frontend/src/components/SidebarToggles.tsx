interface Props {
  circlesOpen: boolean
  infoOpen: boolean
  onToggleCircles: () => void
  onToggleInfo: () => void
}

interface ToggleProps {
  side: 'left' | 'right'
  open: boolean
  label: string
  onClick: () => void
}

function SidebarToggle({ side, open, label, onClick }: ToggleProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`sidebar-toggle sidebar-toggle--${side}${open ? ' is-open' : ''}`}
      aria-label={open ? `Close ${label}` : `Open ${label}`}
      aria-expanded={open}
      title={open ? `Close ${label}` : `Open ${label}`}
    >
      <svg className="sidebar-toggle__glyph" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="4" width="18" height="16" rx="1" />
        <path d={side === 'left' ? 'M9 4v16' : 'M15 4v16'} />
        <rect className="sidebar-toggle__pane" x={side === 'left' ? 4 : 16} y="5" width="4" height="14" />
      </svg>
    </button>
  )
}

/**
 * Inset panel toggles pinned to the outer top corners of the workspace.
 * They replace the former app header, so the panels reach the top of the window.
 */
export default function SidebarToggles({ circlesOpen, infoOpen, onToggleCircles, onToggleInfo }: Props) {
  return (
    <div className="sidebar-toggle-rail">
      <SidebarToggle side="left" open={circlesOpen} label="circles sidebar" onClick={onToggleCircles} />
      <SidebarToggle side="right" open={infoOpen} label="workspace sidebar" onClick={onToggleInfo} />
    </div>
  )
}
