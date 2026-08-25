import { useState, useCallback, useEffect } from 'react'
import type { CSSProperties, PointerEvent as ReactPointerEvent } from 'react'
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

const LAYOUT_PREFERENCES_KEY = 'enoxian.layout.v3'

interface LayoutPreferences {
  leftPanelOpen: boolean
  leftPanelWidth: number
  rightPanelOpen: boolean
  rightPanelTab: RightPanelTab
  rightPanelWidth: number
}

const DEFAULT_LAYOUT_PREFERENCES: LayoutPreferences = {
  leftPanelOpen: true,
  leftPanelWidth: 280,
  rightPanelOpen: true,
  rightPanelTab: 'members',
  rightPanelWidth: 340,
}

function loadLayoutPreferences(): LayoutPreferences {
  try {
    const saved = JSON.parse(localStorage.getItem(LAYOUT_PREFERENCES_KEY) ?? '{}')
    const tabs: RightPanelTab[] = ['members', 'tasks', 'workspace']
    const savedTab = saved.rightPanelTab === 'files' || saved.rightPanelTab === 'changes'
      ? 'workspace'
      : saved.rightPanelTab
    return {
      leftPanelOpen: saved.leftPanelOpen !== false,
      leftPanelWidth: Math.min(360, Math.max(220, Number(saved.leftPanelWidth) || 280)),
      rightPanelOpen: saved.rightPanelOpen !== false,
      rightPanelTab: tabs.includes(savedTab) ? savedTab : 'members',
      rightPanelWidth: Math.min(560, Math.max(280, Number(saved.rightPanelWidth) || 340)),
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
  const [compactLayout, setCompactLayout] = useState(() => window.matchMedia('(max-width: 960px)').matches)
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
    const query = window.matchMedia('(max-width: 960px)')
    const update = () => setCompactLayout(query.matches)
    update()
    query.addEventListener('change', update)
    return () => query.removeEventListener('change', update)
  }, [])

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

  const toggleCircles = () => {
    if (window.matchMedia('(max-width: 960px)').matches) {
      toggle('circles')
      return
    }
    setMobileDrawer(null)
    setLayoutPreferences(current => ({
      ...current,
      leftPanelOpen: !current.leftPanelOpen,
    }))
  }

  const resizeLeftPanel = useCallback((nextWidth: number) => {
    setLayoutPreferences(current => ({
      ...current,
      leftPanelWidth: Math.min(360, Math.max(220, nextWidth)),
    }))
  }, [])

  const startLeftPanelResize = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    const startX = event.clientX
    const startWidth = layoutPreferences.leftPanelWidth
    document.body.classList.add('is-resizing-sidebar')

    const move = (moveEvent: PointerEvent) => resizeLeftPanel(startWidth + moveEvent.clientX - startX)
    const finish = () => {
      document.body.classList.remove('is-resizing-sidebar')
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', finish)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish)
    window.addEventListener('pointercancel', finish)
  }, [layoutPreferences.leftPanelWidth, resizeLeftPanel])

  const resizeRightPanel = useCallback((nextWidth: number) => {
    setLayoutPreferences(current => ({
      ...current,
      rightPanelWidth: Math.min(560, Math.max(280, nextWidth)),
    }))
  }, [])

  const startRightPanelResize = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    const startX = event.clientX
    const startWidth = layoutPreferences.rightPanelWidth
    document.body.classList.add('is-resizing-sidebar')

    const move = (moveEvent: PointerEvent) => resizeRightPanel(startWidth + startX - moveEvent.clientX)
    const finish = () => {
      document.body.classList.remove('is-resizing-sidebar')
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', finish)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish)
    window.addEventListener('pointercancel', finish)
  }, [layoutPreferences.rightPanelWidth, resizeRightPanel])

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
        <div
          className={`app-shell relative z-10 grid drawer-${mobileDrawer ?? 'none'}${layoutPreferences.leftPanelOpen ? ' left-panel-open' : ''}${layoutPreferences.rightPanelOpen ? ' right-panel-open' : ''}${isVoid ? ' app-shell--void' : ''}`}
          style={{
            '--left-panel-width': `${layoutPreferences.leftPanelWidth}px`,
            '--right-panel-width': `${layoutPreferences.rightPanelWidth}px`,
          } as CSSProperties}
        >
          <Header
            mobileDrawer={mobileDrawer}
            circlesOpen={compactLayout ? mobileDrawer === 'circles' : layoutPreferences.leftPanelOpen}
            infoOpen={compactLayout ? mobileDrawer === 'info' : layoutPreferences.rightPanelOpen}
            onToggleCircles={toggleCircles}
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

          <div
            className="left-panel-resizer"
            role="separator"
            aria-label="Resize circles sidebar"
            aria-orientation="vertical"
            aria-valuemin={220}
            aria-valuemax={360}
            aria-valuenow={layoutPreferences.leftPanelWidth}
            tabIndex={0}
            onPointerDown={startLeftPanelResize}
            onKeyDown={event => {
              if (event.key === 'ArrowLeft') { event.preventDefault(); resizeLeftPanel(layoutPreferences.leftPanelWidth - 20) }
              if (event.key === 'ArrowRight') { event.preventDefault(); resizeLeftPanel(layoutPreferences.leftPanelWidth + 20) }
              if (event.key === 'Home') { event.preventDefault(); resizeLeftPanel(220) }
              if (event.key === 'End') { event.preventDefault(); resizeLeftPanel(360) }
            }}
          ><span aria-hidden="true" /></div>

          <div className="desktop-chat">
            {!compactLayout && (
              <>
                <div className={`workspace-view workspace-view--chat${selectedFile ? '' : ' is-active'}`} aria-hidden={!!selectedFile}>
                  <ChatPanel variant="main" hideActiveCircleGlyph={!!ritual} />
                </div>
                {selectedFile && (
                  <div key={selectedFile} className="workspace-view workspace-view--file is-active">
                    <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
                  </div>
                )}
              </>
            )}
          </div>

          <div
            className="right-panel-resizer"
            role="separator"
            aria-label="Resize workspace sidebar"
            aria-orientation="vertical"
            aria-valuemin={280}
            aria-valuemax={560}
            aria-valuenow={layoutPreferences.rightPanelWidth}
            tabIndex={0}
            onPointerDown={startRightPanelResize}
            onKeyDown={event => {
              if (event.key === 'ArrowLeft') { event.preventDefault(); resizeRightPanel(layoutPreferences.rightPanelWidth + 20) }
              if (event.key === 'ArrowRight') { event.preventDefault(); resizeRightPanel(layoutPreferences.rightPanelWidth - 20) }
              if (event.key === 'Home') { event.preventDefault(); resizeRightPanel(280) }
              if (event.key === 'End') { event.preventDefault(); resizeRightPanel(560) }
            }}
          ><span aria-hidden="true" /></div>

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
              {compactLayout && (
                <>
                  <div className={`workspace-view workspace-view--chat${selectedFile ? '' : ' is-active'}`} aria-hidden={!!selectedFile}>
                    <ChatPanel variant="main" hideActiveCircleGlyph={!!ritual} />
                  </div>
                  {selectedFile && (
                    <div key={selectedFile} className="workspace-view workspace-view--file is-active">
                      <EditorPanel filePath={selectedFile} onBack={() => setSelectedFile(null)} />
                    </div>
                  )}
                </>
              )}
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
