import { useRef, useState, useCallback } from 'react'
import { AppProvider } from './context/AppContext'
import ThreeBackground, { type SceneHandle } from './components/ThreeBackground'
import Header from './components/Header'
import ChatPanel from './components/ChatPanel'
import EditorPanel from './components/EditorPanel'
import RightPanel from './components/RightPanel'
import './styles/globals.css'

function Layout() {
  const sceneRef = useRef<SceneHandle>(null)
  const [selectedFile, setSelectedFile] = useState<string | null>(null)

  const onMessage = useCallback(() => {
    sceneRef.current?.pulse(
      (Math.random() - 0.5) * 6,
      (Math.random() - 0.5) * 6,
    )
  }, [])

  const onFileSelect = useCallback((path: string) => {
    setSelectedFile(path)
  }, [])

  return (
    <>
      <ThreeBackground ref={sceneRef} />

      <div className="relative z-10 h-screen grid"
           style={{ gridTemplateColumns: '320px 1fr 300px', gridTemplateRows: '60px 1fr' }}>
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
