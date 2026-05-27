import { useApp } from '../context/AppContext'
import CircleManager from './CircleManager'

export default function Header() {
  const { status } = useApp()

  return (
    <header className="app-header border-b-2 border-obsidian bg-alabaster z-[100]
                       flex items-center justify-between gap-4 px-6 min-h-[60px] font-mono text-[11px] uppercase font-bold tracking-widest">
      <div className="flex min-w-0 items-center gap-6">
        <span className="shrink-0 text-obsidian">ENOCHIAN</span>
        <span className="text-slate font-normal">//</span>
        <CircleManager />
      </div>

      <div className="header-status flex items-center justify-end gap-8 text-slate font-normal">
        {status && (
          <>
            <span>AGENT: <span className="text-obsidian font-bold">{status.agent_id}</span></span>
            <span>DOCS: <span className="text-obsidian font-bold">{status.docs}</span></span>
          </>
        )}
        <span className="text-obsidian">YJS CRDT // SYNC: ACTIVE</span>
      </div>
    </header>
  )
}
