import { useApp } from '../context/AppContext'

export default function Header() {
  const { status, circles, activeCircleId } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

  return (
    <header className="app-header sys-window z-[100] flex items-center justify-between gap-4 px-5 min-h-[48px] font-mono text-[11px] uppercase font-bold">
      <div className="flex items-center gap-4 min-w-0">
        <span className="brand-mark shrink-0">E</span>
        {activeCircle && (
          <span className="font-bold tracking-widest truncate">{activeCircle.circle_name}</span>
        )}
      </div>

      <div className="flex items-center gap-3 text-slate font-normal shrink-0">
        {status && (
          <>
            <span className="sys-badge">{status.agent_id}</span>
            <span className="sys-badge">docs {status.docs}</span>
          </>
        )}
      </div>
    </header>
  )
}
