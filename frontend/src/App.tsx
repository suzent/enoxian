import { useRef, useState, useCallback, useEffect } from 'react'
import { AppProvider } from './context/AppContext'
import ThreeBackground, { type SceneHandle } from './components/ThreeBackground'
import Header from './components/Header'
import ChatPanel from './components/ChatPanel'
import EditorPanel from './components/EditorPanel'
import RightPanel from './components/RightPanel'
import { useApp } from './context/AppContext'
import './styles/globals.css'

function Layout() {
  const sceneRef = useRef<SceneHandle>(null)
  const { activeCircleId } = useApp()
  const [selectedFile, setSelectedFile] = useState<string | null>(null)

  useEffect(() => {
    setSelectedFile(null)
  }, [activeCircleId])

  const onMessage = useCallback(() => {
    sceneRef.current?.pulse(
      (Math.random() - 0.5) * 6,
      (Math.random() - 0.5) * 6,
    )
  }, [])

  const onFileSelect = useCallback((path: string | null) => {
    setSelectedFile(path)
  }, [])

  return (
    <>
      <ThreeBackground ref={sceneRef} />

      <div className="app-shell relative z-10 grid">
        <Header />
        <ChatPanel onMessage={onMessage} />
        <EditorPanel filePath={selectedFile} />
        <RightPanel onFileSelect={onFileSelect} selectedFile={selectedFile} />
      </div>
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
