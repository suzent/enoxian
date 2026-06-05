import { useState, useCallback, useEffect } from 'react'
import { AppProvider } from './context/AppContext'
import ChatPanel from './components/ChatPanel'
import EditorPanel from './components/EditorPanel'
import RightPanel from './components/RightPanel'
import CircleSidebar from './components/CircleSidebar'
import Header from './components/Header'
import VoidOverlay from './components/VoidOverlay'
import LandingPage from './components/LandingPage'
import RitualTransition, { type RitualMode } from './components/RitualTransition'
import { useApp } from './context/AppContext'
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
        <div className={`app-shell relative z-10 grid drawer-${mobileDrawer ?? 'none'}${isVoid ? ' app-shell--void' : ''}`}>
          <Header
            mobileDrawer={mobileDrawer}
            onToggleCircles={() => toggle('circles')}
            onToggleInfo={() => toggle('info')}
          />

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
