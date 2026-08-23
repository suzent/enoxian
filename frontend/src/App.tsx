import { useState, useCallback, useEffect } from 'react'
import { AppProvider } from './context/AppContext'
import ChatPanel from './components/ChatPanel'
import EditorPanel from './components/EditorPanel'
import RightPanel, { type RightPanelTab } from './components/RightPanel'
import CircleSidebar from './components/CircleSidebar'
import Header from './components/Header'
import LandingPage from './components/LandingPage'
import RitualTransition, { type RitualMode } from './components/RitualTransition'
import { useApp } from './context/AppContext'
import { leaveCircle } from './api'
import { Trash2 } from 'lucide-react'
import './styles/globals.css'

type MobileDrawer = 'circles' | 'info' | null

const LAYOUT_PREFERENCES_KEY = 'enoxian.layout.v1'

interface LayoutPreferences {
  rightPanelOpen: boolean
  rightPanelTab: RightPanelTab
}

const DEFAULT_LAYOUT_PREFERENCES: LayoutPreferences = {
  rightPanelOpen: false,
  rightPanelTab: 'members',
}

function loadLayoutPreferences(): LayoutPreferences {
  try {
    const saved = JSON.parse(localStorage.getItem(LAYOUT_PREFERENCES_KEY) ?? '{}')
    const tabs: RightPanelTab[] = ['members', 'tasks', 'files', 'changes']
    return {
      rightPanelOpen: saved.rightPanelOpen === true,
      rightPanelTab: tabs.includes(saved.rightPanelTab) ? saved.rightPanelTab : 'members',
    }
  } catch {
    return DEFAULT_LAYOUT_PREFERENCES
  }
}

function Layout() {
  const { activeCircleId, circles, circlesLoaded, circlesError, status, reloadCircles } = useApp()

  const [selectedFile, setSelectedFile] = useState<string | null>(null)
  const [ritual, setRitual] = useState<{ mode: RitualMode; label?: string } | null>(null)
  const [showLanding, setShowLanding] = useState(false)
  const [revealing, setRevealing] = useState(false)
  const [layoutPreferences, setLayoutPreferences] = useState(loadLayoutPreferences)
  const [mobileDrawer, setMobileDrawer] = useState<MobileDrawer>(null)
  const [confirmRemovedLeave, setConfirmRemovedLeave] = useState(false)
  const [removedLeaveBusy, setRemovedLeaveBusy] = useState(false)
  const [removedLeaveError, setRemovedLeaveError] = useState<string | null>(null)

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
    setConfirmRemovedLeave(false)
    setRemovedLeaveBusy(false)
    setRemovedLeaveError(null)
  }, [activeCircleId])

  useEffect(() => {
    if (selectedFile) setMobileDrawer(null)
  }, [selectedFile])

  useEffect(() => {
    try { localStorage.setItem(LAYOUT_PREFERENCES_KEY, JSON.stringify(layoutPreferences)) } catch {}
  }, [layoutPreferences])

  useEffect(() => {
    if (!mobileDrawer) return
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMobileDrawer(null)
    }
    window.addEventListener('keydown', closeOnEscape)
    return () => window.removeEventListener('keydown', closeOnEscape)
  }, [mobileDrawer])

  const onFileSelect = useCallback((path: string | null) => {
    setSelectedFile(path)
  }, [])

  const toggle = (drawer: MobileDrawer) =>
    setMobileDrawer(d => d === drawer ? null : drawer)

  const toggleInfo = () => {
    if (window.matchMedia('(max-width: 960px)').matches) {
      toggle('info')
      return
    }
    setMobileDrawer(null)
    setLayoutPreferences(current => ({
      ...current,
      rightPanelOpen: !current.rightPanelOpen,
    }))
  }

  const handleRemovedLeave = async () => {
    if (!activeCircleId || removedLeaveBusy) return
    if (!confirmRemovedLeave) {
      setConfirmRemovedLeave(true)
      setRemovedLeaveError(null)
      return
    }
    setRemovedLeaveBusy(true)
    setRemovedLeaveError(null)
    try {
      await leaveCircle(activeCircleId)
      await reloadCircles()
    } catch (error) {
      setRemovedLeaveError(error instanceof Error ? error.message : 'Unable to remove Circle')
      setRemovedLeaveBusy(false)
    }
  }

  return (
    <>
      <RitualTransition ritual={ritual} onComplete={() => setRitual(null)} />

      {circlesLoaded && circlesError && circles.length === 0 && (
        <main className="fixed inset-0 z-20 grid place-items-center bg-alabaster p-6">
          <div className="sys-window max-w-md p-6 text-center font-mono">
            <strong className="block text-sm">ENOXIAN IS NOT RESPONDING</strong>
            <p className="my-3 text-[11px] text-slate">{circlesError}</p>
            <button type="button" className="enox-btn px-3 py-2 text-[10px]" onClick={reloadCircles}>
              TRY AGAIN
            </button>
          </div>
        </main>
      )}

      {showLanding && !circlesError && <LandingPage onEntered={handleEntered} />}

      {revealing && (
        <div className="app-reveal-overlay" onAnimationEnd={() => setRevealing(false)} />
      )}

      {circles.length > 0 && (
        <div className={`app-shell relative z-10 grid drawer-${mobileDrawer ?? 'none'}${layoutPreferences.rightPanelOpen ? ' right-panel-open' : ''}${isVoid ? ' app-shell--void' : ''}`}>
          <Header
            mobileDrawer={mobileDrawer}
            infoOpen={layoutPreferences.rightPanelOpen || mobileDrawer === 'info'}
            onToggleCircles={() => toggle('circles')}
            onToggleInfo={toggleInfo}
          />

          {status?.removed && (
            <div className="circle-removed-notice" role="alert">
              <strong>ACCESS REVOKED</strong>
              <span>This device was removed from {activeCircle?.circle_name ?? 'this Circle'}.</span>
              <small>Circle sync and member actions are disabled. Existing local files remain on this device.</small>
              {confirmRemovedLeave ? (
                <div className="circle-removed-notice__confirm">
                  <small>This removes the Circle configuration from this device. Workspace files are untouched.</small>
                  <div>
                    <button
                      type="button"
                      onClick={() => setConfirmRemovedLeave(false)}
                      disabled={removedLeaveBusy}
                      className="circle-removed-notice__cancel"
                    >
                      CANCEL
                    </button>
                    <button
                      type="button"
                      onClick={handleRemovedLeave}
                      disabled={removedLeaveBusy}
                      className="circle-removed-notice__remove"
                    >
                      <Trash2 size={14} aria-hidden="true" />
                      {removedLeaveBusy ? 'REMOVING...' : 'REMOVE'}
                    </button>
                  </div>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={handleRemovedLeave}
                  className="circle-removed-notice__remove"
                >
                  <Trash2 size={14} aria-hidden="true" />
                  REMOVE FROM THIS DEVICE
                </button>
              )}
              {removedLeaveError && <small className="circle-removed-notice__error">{removedLeaveError}</small>}
            </div>
          )}

          <CircleSidebar
            onRitual={(mode, label) => setRitual({ mode, label })}
            ritualCircleName={ritual?.label}
          />

          <div className="desktop-chat">
            {selectedFile
              ? <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
              : <ChatPanel variant="main" hideActiveCircleGlyph={!!ritual} />}
          </div>

          <RightPanel
            onFileSelect={onFileSelect}
            selectedFile={selectedFile}
            activeTab={layoutPreferences.rightPanelTab}
            onActiveTabChange={rightPanelTab => {
              setLayoutPreferences(current => ({ ...current, rightPanelTab }))
            }}
          />

          <div className="mobile-main-area">
            <div className="mobile-main">
              {selectedFile
                ? <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
              : <ChatPanel variant="main" hideActiveCircleGlyph={!!ritual} />}
            </div>
            <div className={`mobile-drawer mobile-drawer--left${mobileDrawer === 'circles' ? ' open' : ''}`}>
              <CircleSidebar
                onRitual={(mode, label) => { setRitual({ mode, label }); setMobileDrawer(null) }}
                ritualCircleName={ritual?.label}
              />
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
