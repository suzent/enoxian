import { useState, useCallback, useEffect } from 'react'
import { AppProvider } from './context/AppContext'
import ChatPanel from './components/ChatPanel'
import EditorPanel from './components/EditorPanel'
import RightPanel from './components/RightPanel'
import CircleSidebar from './components/CircleSidebar'
import VoidOverlay from './components/VoidOverlay'
import LandingPage from './components/LandingPage'
import RitualTransition, { type RitualMode } from './components/RitualTransition'
import { useApp } from './context/AppContext'
import { Menu, PanelRight } from 'lucide-react'
import './styles/globals.css'

type MobileDrawer = 'circles' | 'info' | null

function Layout() {
  const { activeCircleId, circles, circlesLoaded } = useApp()

  const [selectedFile, setSelectedFile] = useState<string | null>(null)
  const [ritual, setRitual] = useState<{ mode: RitualMode; label?: string } | null>(null)
  const [showLanding, setShowLanding] = useState(false)
  const [revealing, setRevealing] = useState(false)
  const [mobileDrawer, setMobileDrawer] = useState<MobileDrawer>(null)

  const handleEntered = useCallback(() => {
    setShowLanding(false)
    setRevealing(true)
  }, [])

  useEffect(() => {
    if (!circlesLoaded) return
    if (circles.length === 0) {
      setShowLanding(true)
    } else if (!revealing) {
      setShowLanding(false)
    }
  }, [circlesLoaded, circles.length])

  const activeCircle = circles.find(c => c.circle_id === activeCircleId)
  const isVoid = activeCircle?.disabled ?? false

  useEffect(() => {
    setSelectedFile(null)
  }, [activeCircleId])

  useEffect(() => {
    if (selectedFile) setMobileDrawer(null)
  }, [selectedFile])

  const onFileSelect = useCallback((path: string | null) => {
    setSelectedFile(path)
  }, [])

  const toggle = (drawer: MobileDrawer) =>
    setMobileDrawer(d => d === drawer ? null : drawer)

  return (
    <>
      <RitualTransition ritual={ritual} onComplete={() => setRitual(null)} />

      {showLanding && <LandingPage onEntered={handleEntered} />}

      {revealing && (
        <div className="app-reveal-overlay" onAnimationEnd={() => setRevealing(false)} />
      )}

      {isVoid && activeCircle && (
        <VoidOverlay circleName={activeCircle.circle_name} />
      )}

      {circles.length > 0 && (
        <div className={`app-shell relative z-10 grid${isVoid ? ' app-shell--void' : ''}`}>
          {/* Mobile nav (replaces header) */}
          <div className="mobile-nav app-header sys-window z-[100] items-center justify-between gap-4 px-5 min-h-[48px] font-mono text-[11px] uppercase font-bold">
            <div className="flex items-center gap-3 min-w-0">
              <span className="brand-mark shrink-0">E</span>
              <button
                onClick={() => toggle('circles')}
                className={`mobile-header-btn${mobileDrawer === 'circles' ? ' active' : ''}`}
              >
                <Menu size={18} strokeWidth={2.5} />
              </button>
              {activeCircle && (
                <span className="font-bold tracking-widest truncate">{activeCircle.circle_name}</span>
              )}
            </div>
            <button
              onClick={() => toggle('info')}
              className={`mobile-header-btn${mobileDrawer === 'info' ? ' active' : ''}`}
            >
              <PanelRight size={18} strokeWidth={2.5} />
            </button>
          </div>

          {/* Desktop sidebar */}
          <CircleSidebar onRitual={(mode, label) => setRitual({ mode, label })} />

          {/* Desktop chat */}
          <div className="desktop-chat">
            {selectedFile
              ? <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
              : <ChatPanel variant="main" />}
          </div>

          {/* Desktop right panel */}
          <RightPanel onFileSelect={onFileSelect} selectedFile={selectedFile} />

          {/* Mobile: chat + drawers in a shared container */}
          <div className="mobile-main-area">
            <div className="mobile-main">
              {selectedFile
                ? <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
                : <ChatPanel variant="main" />}
            </div>
            <div className={`mobile-drawer mobile-drawer--left${mobileDrawer === 'circles' ? ' open' : ''}`}>
              <CircleSidebar onRitual={(mode, label) => { setRitual({ mode, label }); setMobileDrawer(null) }} />
            </div>
            <div className={`mobile-drawer mobile-drawer--right${mobileDrawer === 'info' ? ' open' : ''}`}>
              <RightPanel onFileSelect={onFileSelect} selectedFile={selectedFile} />
            </div>
          </div>
        </div>
      )}
    </>
  )
}

export default function App() {
  return (
    <AppProvider>
      <Layout />
    </AppProvider>
  )
}
