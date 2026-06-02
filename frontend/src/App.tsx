import { useState, useCallback, useEffect } from 'react'
import { AppProvider } from './context/AppContext'
import Header from './components/Header'
import ChatPanel from './components/ChatPanel'
import EditorPanel from './components/EditorPanel'
import RightPanel from './components/RightPanel'
import CircleSidebar from './components/CircleSidebar'
import VoidOverlay from './components/VoidOverlay'
import LandingPage from './components/LandingPage'
import RitualTransition, { type RitualMode } from './components/RitualTransition'
import { useApp } from './context/AppContext'
import './styles/globals.css'

function Layout() {
  const { activeCircleId, circles } = useApp()

  const [selectedFile, setSelectedFile] = useState<string | null>(null)
  const [ritual, setRitual] = useState<{ mode: RitualMode; label?: string } | null>(null)
  const [showLanding, setShowLanding] = useState(circles.length === 0)

  useEffect(() => {
    if (circles.length === 0) {
      setShowLanding(true)
    }
  }, [circles.length])

  const activeCircle = circles.find(c => c.circle_id === activeCircleId)
  const isVoid = activeCircle?.disabled ?? false

  useEffect(() => {
    setSelectedFile(null)
  }, [activeCircleId])

  const onFileSelect = useCallback((path: string | null) => {
    setSelectedFile(path)
  }, [])

  return (
    <>
      <RitualTransition ritual={ritual} onComplete={() => setRitual(null)} />

      {showLanding && (
        <LandingPage onEntered={() => setShowLanding(false)} />
      )}

      {isVoid && activeCircle && (
        <VoidOverlay circleName={activeCircle.circle_name} />
      )}

      {circles.length > 0 && (
        <div className={`app-shell relative z-10 grid${isVoid ? ' app-shell--void' : ''}`}>
        <Header />
        <CircleSidebar onRitual={(mode, label) => setRitual({ mode, label })} />
        {selectedFile ? (
          <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
        ) : (
          <ChatPanel variant="main" />
        )}
        <RightPanel onFileSelect={onFileSelect} selectedFile={selectedFile} />
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
