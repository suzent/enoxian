import { useApp } from '../context/AppContext'
import CircleManager from './CircleManager'
import type { RitualMode } from './RitualTransition'

interface Props {
  onRitual?: (mode: RitualMode, label?: string) => void
}

export default function Header({ onRitual }: Props) {
  const { status } = useApp()

  return (
    <header className="app-header sys-window z-[100] flex items-center justify-between gap-4 px-5 min-h-[64px] font-mono text-[11px] uppercase font-bold">
      <div className="flex min-w-0 items-center gap-6">
        <span className="brand-mark shrink-0">E</span>
        <CircleManager onRitual={onRitual} />
      </div>

      <div className="header-status flex items-center justify-end gap-3 text-slate font-normal">
        {status && (
          <>
            <span className="sys-badge">agent {status.agent_id}</span>
            <span className="sys-badge">docs {status.docs}</span>
          </>
        )}
        <span className="sys-badge">yjs synced</span>
      </div>
    </header>
  )
}
