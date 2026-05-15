import { useApp } from '../context/AppContext'

export default function Header() {
  const { circles, activeCircleId, setActiveCircleId, status } = useApp()

  return (
    <header className="col-span-3 row-start-1 border-b-2 border-obsidian bg-alabaster z-10
                       flex items-center justify-between px-6 h-[60px] font-mono text-[11px] uppercase font-bold tracking-widest">
      <div className="flex items-center gap-6">
        <span className="text-obsidian">ENOCHIAN</span>
        <span className="text-slate font-normal">//</span>
        <select
          value={activeCircleId ?? ''}
          onChange={e => setActiveCircleId(e.target.value)}
          className="bg-transparent border border-obsidian font-mono text-[11px] uppercase font-bold
                     px-2 py-1 cursor-pointer focus:outline-none appearance-none pr-6"
          style={{ backgroundImage: 'none' }}
        >
          {circles.length === 0 && <option value="">NO CIRCLES</option>}
          {circles.map(c => (
            <option key={c.circle_id} value={c.circle_id}>{c.circle_name}</option>
          ))}
        </select>
      </div>

      <div className="flex items-center gap-8 text-slate font-normal">
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
