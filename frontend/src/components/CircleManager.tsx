import { useState } from 'react'
import { initCircle, enterCircle, enableCircle, disableCircle, leaveCircle } from '../api'
import { useApp } from '../context/AppContext'

export default function CircleManager() {
  const { circles, activeCircleId, setActiveCircleId, reloadCircles } = useApp()
  const [dropdownOpen, setDropdownOpen] = useState(false)
  const [modal, setModal] = useState<'init' | 'enter' | 'leave' | null>(null)

  // Form states
  const [initName, setInitName] = useState('')
  const [enterTarget, setEnterTarget] = useState('')
  const [errorMsg, setErrorMsg] = useState('')

  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

  const handleInit = async (e: React.FormEvent) => {
    e.preventDefault()
    setErrorMsg('')
    try {
      const res = await initCircle(initName)
      await reloadCircles()
      if (res.circle_id) setActiveCircleId(res.circle_id)
      setModal(null)
      setInitName('')
    } catch (err: any) {
      setErrorMsg(err.message)
    }
  }

  const handleEnter = async (e: React.FormEvent) => {
    e.preventDefault()
    setErrorMsg('')
    try {
      await enterCircle(enterTarget)
      await reloadCircles()
      setModal(null)
      setEnterTarget('')
    } catch (err: any) {
      setErrorMsg(err.message)
    }
  }

  const handleLeave = async () => {
    if (!activeCircleId) return
    try {
      await leaveCircle(activeCircleId)
      await reloadCircles()
      setModal(null)
    } catch (err: any) {
      alert(`Error leaving circle: ${err.message}`)
    }
  }

  return (
    <div className="relative flex items-center">
      <div className="flex items-stretch shadow-[2px_2px_0px_#111]">
        {/* Circle Selector Button */}
        <button
          onClick={() => setDropdownOpen(!dropdownOpen)}
          className="bg-alabaster border-2 border-obsidian font-mono text-[11px] uppercase font-bold px-3 py-1 cursor-pointer hover:bg-obsidian/5 transition-colors min-w-[200px] text-left flex justify-between items-center"
        >
          <span>
            {activeCircle ? activeCircle.circle_name : 'NO CIRCLE SELECTED'}
          </span>
          <span className="text-[9px] ml-4">▼</span>
        </button>

        {/* State Toggle */}
        {activeCircle && (
          <div 
            onClick={async () => {
              if (activeCircle.disabled) {
                await enableCircle(activeCircle.circle_id);
              } else {
                await disableCircle(activeCircle.circle_id);
              }
              await reloadCircles();
            }}
            className={`flex items-center justify-center gap-2 w-[120px] px-2 py-1 border-2 border-l-0 border-obsidian cursor-pointer select-none transition-colors ${
              !activeCircle.disabled 
                ? 'bg-obsidian text-alabaster hover:text-slate' 
                : 'bg-alabaster text-slate hover:bg-slate/10'
            }`}
            title={activeCircle.disabled ? 'Summon the circle into manifest reality.' : 'Return the circle to the void.'}
          >
            {/* The Void Monolith Glyph */}
            <span className="font-mono text-[12px] font-bold">
              {!activeCircle.disabled ? '[█]' : '{∅}'}
            </span>
            {/* Enochian Theme Text */}
            <span className="font-mono text-[10px] font-bold tracking-widest">
              {!activeCircle.disabled ? 'MANIFEST' : 'VOID'}
            </span>
          </div>
        )}
      </div>

      {/* Subtle Leave Button */}
      {activeCircle && (
        <button
          onClick={() => setModal('leave')}
          className="ml-2 px-2 text-slate hover:text-red-600 transition-colors flex items-center font-bold text-[10px] uppercase"
          title="Leave Circle"
        >
          LEAVE ×
        </button>
      )}

      {/* Dropdown Menu */}
      {dropdownOpen && (
        <div className="absolute top-full left-0 mt-2 w-[280px] bg-alabaster border-2 border-obsidian z-50 shadow-[4px_4px_0px_#111] text-[11px]">
          {circles.length > 0 && (
            <div className="px-3 py-2 border-b-2 border-obsidian font-bold text-alabaster bg-obsidian text-[10px] tracking-widest">
              SWITCH CIRCLE
            </div>
          )}
          {circles.map(c => (
            <button
              key={c.circle_id}
              onClick={() => {
                setActiveCircleId(c.circle_id)
                setDropdownOpen(false)
              }}
              className={`w-full text-left px-4 py-2 border-b border-obsidian/20 hover:bg-obsidian hover:text-alabaster transition-colors ${c.circle_id === activeCircleId ? 'font-bold bg-obsidian/5' : ''}`}
            >
              {c.circle_name} {c.disabled && <span className="text-red-500 font-normal ml-2">(DISABLED)</span>}
            </button>
          ))}
          


          <div className="px-3 py-2 border-y-2 border-obsidian font-bold text-alabaster bg-obsidian text-[10px] tracking-widest mt-1">
            GLOBAL ACTIONS
          </div>
          <button
            onClick={() => { setModal('init'); setDropdownOpen(false) }}
            className="w-full text-left px-4 py-2 border-b border-obsidian/20 hover:bg-obsidian hover:text-alabaster transition-colors font-bold"
          >
            [+] INIT NEW CIRCLE
          </button>
          <button
            onClick={() => { setModal('enter'); setDropdownOpen(false) }}
            className="w-full text-left px-4 py-2 hover:bg-obsidian hover:text-alabaster transition-colors font-bold"
          >
            {'[>]'} ENTER VIA INVITE
          </button>
        </div>
      )}

      {/* Modals */}
      {modal && (
        <div className="fixed inset-0 bg-obsidian/60 z-[100] flex items-center justify-center backdrop-blur-sm">
          <div className="bg-alabaster border-2 border-obsidian p-6 w-[400px] shadow-[8px_8px_0px_#111] relative text-[11px] uppercase font-mono">
            <button
              onClick={() => setModal(null)}
              className="absolute top-2 right-3 text-obsidian hover:text-red-600 font-bold text-xl leading-none"
            >
              ×
            </button>
            
            {modal === 'init' && (
              <form onSubmit={handleInit}>
                <h2 className="text-[14px] font-bold mb-4 border-b-2 border-obsidian pb-2">INIT NEW CIRCLE</h2>
                {errorMsg && <div className="text-red-600 mb-2 font-bold bg-red-100 p-2 border border-red-600">{errorMsg}</div>}
                <div className="mb-6">
                  <label className="block text-slate font-bold mb-1 tracking-widest">CIRCLE NAME</label>
                  <input
                    type="text"
                    required
                    value={initName}
                    onChange={e => setInitName(e.target.value)}
                    className="w-full border-2 border-obsidian bg-transparent px-3 py-2 outline-none focus:bg-obsidian/5 font-bold"
                    placeholder="e.g. project-alpha"
                  />
                </div>
                <button type="submit" className="w-full bg-obsidian text-alabaster py-3 font-bold hover:bg-slate transition-colors border-2 border-obsidian shadow-[2px_2px_0px_#111] active:translate-y-px active:translate-x-px active:shadow-none">
                  CREATE CIRCLE
                </button>
              </form>
            )}

            {modal === 'enter' && (
              <form onSubmit={handleEnter}>
                <h2 className="text-[14px] font-bold mb-4 border-b-2 border-obsidian pb-2">ENTER CIRCLE</h2>
                {errorMsg && <div className="text-red-600 mb-2 font-bold bg-red-100 p-2 border border-red-600">{errorMsg}</div>}
                <div className="mb-6">
                  <label className="block text-slate font-bold mb-1 tracking-widest">INVITE URI</label>
                  <textarea
                    required
                    value={enterTarget}
                    onChange={e => setEnterTarget(e.target.value)}
                    className="w-full border-2 border-obsidian bg-transparent px-3 py-2 outline-none focus:bg-obsidian/5 h-24 resize-none font-bold"
                    placeholder="enochian://..."
                  />
                </div>
                <button type="submit" className="w-full bg-obsidian text-alabaster py-3 font-bold hover:bg-slate transition-colors border-2 border-obsidian shadow-[2px_2px_0px_#111] active:translate-y-px active:translate-x-px active:shadow-none">
                  JOIN CIRCLE
                </button>
              </form>
            )}

            {modal === 'leave' && (
              <div>
                <h2 className="text-[14px] font-bold mb-4 border-b-2 border-red-600 pb-2 text-red-600">LEAVE CIRCLE</h2>
                <p className="mb-6 leading-relaxed font-bold text-obsidian/80">
                  ARE YOU SURE YOU WANT TO LEAVE <span className="text-obsidian text-[12px] bg-obsidian/10 px-1">"{activeCircle?.circle_name}"</span>?<br/><br/>
                  THIS WILL REMOVE ALL LOCAL CONFIGURATION. YOUR WORKSPACE FILES WILL BE UNTOUCHED.
                </p>
                <div className="flex gap-4">
                  <button onClick={() => setModal(null)} className="flex-1 border-2 border-obsidian py-2 font-bold hover:bg-obsidian/5 transition-colors shadow-[2px_2px_0px_#111] active:translate-y-px active:translate-x-px active:shadow-none">
                    CANCEL
                  </button>
                  <button onClick={handleLeave} className="flex-1 bg-red-600 border-2 border-red-600 text-white font-bold py-2 hover:bg-red-700 transition-colors shadow-[2px_2px_0px_#991b1b] active:translate-y-px active:translate-x-px active:shadow-none">
                    CONFIRM LEAVE
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
