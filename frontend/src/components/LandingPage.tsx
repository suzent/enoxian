/**
 * LandingPage — first-run screen shown when the user has no circles.
 *
 * Combines:
 *  - A full-screen Three.js cyber-angel scene (dithered, mix-blend-mode:multiply)
 *  - A centered sys-window overlay for identity setup + circle init/enter
 */
import { useEffect, useRef, useState, useCallback } from 'react'
import {
  getIdentity,
  setIdentity,
  linkDevice,
  initCircle,
  enterCircle,
  createUserIdentity,
} from '../api'
import { useApp } from '../context/AppContext'
import { BRAND_LOGO_SRC } from '../lib/brand'

// ── Types ─────────────────────────────────────────────────────────────────────

interface Props {
  onEntered: () => void
}

type UIState = 'setup' | 'init-form' | 'enter-form' | 'mnemonic-backup'

import { buildAngelScene, type AngelScene } from './AngelScene'

export default function LandingPage({ onEntered }: Props) {
  const mountRef = useRef<HTMLDivElement>(null)
  const angelRef = useRef<AngelScene | null>(null)
  const { reloadCircles, setActiveCircleId } = useApp()

  // UI state
  const [uiState, setUIState] = useState<UIState>('setup')
  const [linkExpanded, setLinkExpanded] = useState(false)

  // Identity fields
  const [userName, setUserName] = useState('')
  const [deviceLabel, setDeviceLabel] = useState('')
  const [hasUserKey, setHasUserKey] = useState(false)
  const [isEditingIdentity, setIsEditingIdentity] = useState(false)

  // Init-circle form
  const [circleName, setCircleName] = useState('')
  const [joinPolicy, setJoinPolicy] = useState<'auto' | 'manual'>('auto')

  // Enter-circle form
  const [inviteUri, setInviteUri] = useState('')

  // Link-device form
  const [linkHandle, setLinkHandle] = useState('')
  const [linkMnemonic, setLinkMnemonic] = useState('')
  const [linkSuccess, setLinkSuccess] = useState(false)

  // Mnemonic backup
  const [mnemonic, setMnemonic] = useState('')
  const [pendingCircleId, setPendingCircleId] = useState<string | null>(null)

  // Error / loading / eruption
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [isErupting, setIsErupting] = useState(false)

  // Load identity on mount
  useEffect(() => {
    getIdentity().then(info => {
      if (info.user_handle) setUserName(info.user_handle)
      setDeviceLabel(info.device_label)
      setHasUserKey(info.has_user_key)
      // If identity not yet configured, start in edit mode
      if (!info.user_handle) setIsEditingIdentity(true)
    }).catch(() => { setIsEditingIdentity(true) })
  }, [])

  // Build Three.js scene on mount
  useEffect(() => {
    const mount = mountRef.current
    if (!mount) return
    const angel = buildAngelScene(mount)
    angelRef.current = angel
    return () => {
      angel.dispose()
      angelRef.current = null
    }
  }, [])

  // ── Helpers ─────────────────────────────────────────────────────────────

  async function saveIdentityIfNeeded() {
    try {
      await setIdentity({
        user_handle: userName.trim() || undefined,
        device_label: deviceLabel.trim() || undefined,
      })
    } catch {
      // non-fatal — proceed
    }
  }

  async function triggerEruptionAndComplete(circleId?: string | null) {
    setIsErupting(true)
    angelRef.current?.triggerEruption(async () => {
      await reloadCircles()
      if (circleId) setActiveCircleId(circleId)
      onEntered()
    })
  }

  // ── Actions ──────────────────────────────────────────────────────────────

  const handleInitClick = useCallback(() => {
    setError('')
    setUIState('init-form')
  }, [])

  const handleEnterClick = useCallback(() => {
    setError('')
    setUIState('enter-form')
  }, [])

  const handleBack = useCallback(() => {
    setError('')
    setUIState('setup')
  }, [])

  const handleCreateCircle = useCallback(async () => {
    setError('')
    setLoading(true)
    try {
      await saveIdentityIfNeeded()
      // If no user identity yet, create one first to get mnemonic
      const identity = await getIdentity()
      if (!identity.has_user_key && userName.trim()) {
        const result = await createUserIdentity(userName.trim())
        setMnemonic(result.mnemonic)
        // Proceed to init circle after showing backup screen
        const created = await initCircle(circleName.trim() || 'DEFAULT', userName.trim() || undefined, joinPolicy)
        setPendingCircleId(created.circle_id ?? null)
        setUIState('mnemonic-backup')
        return
      }
      const created = await initCircle(circleName.trim() || 'DEFAULT', userName.trim() || undefined, joinPolicy)
      triggerEruptionAndComplete(created.circle_id)
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [circleName, joinPolicy, userName, deviceLabel])

  const handleJoinCircle = useCallback(async () => {
    setError('')
    setLoading(true)
    try {
      await saveIdentityIfNeeded()
      const entered = await enterCircle(inviteUri.trim(), userName.trim() || undefined)
      triggerEruptionAndComplete(entered.circle_id)
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [inviteUri, userName, deviceLabel])

  const handleLinkDevice = useCallback(async () => {
    setError('')
    setLoading(true)
    try {
      const result = await linkDevice(linkHandle.trim(), linkMnemonic.trim())
      setUserName(result.user_handle)
      setLinkSuccess(true)
      setLinkExpanded(false)
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [linkHandle, linkMnemonic])

  const handleMnemonicConfirmed = useCallback(() => {
    triggerEruptionAndComplete(pendingCircleId)
  }, [pendingCircleId])

  // ── Shared input style ────────────────────────────────────────────────────

  const inputStyle: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 11,
    fontWeight: 700,
    background: '#fff',
    color: '#000',
    border: '1px solid #000',
    outline: 'none',
    padding: '5px 8px',
    width: '100%',
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    minHeight: 28,
  }

  const labelStyle: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 10,
    fontWeight: 800,
    color: '#555',
    textTransform: 'uppercase',
    letterSpacing: '0.1em',
    display: 'block',
    marginBottom: 3,
  }

  const rowStyle: React.CSSProperties = {
    display: 'grid',
    gridTemplateColumns: '90px 1fr',
    gap: 8,
    alignItems: 'center',
    marginBottom: 8,
  }

  const btnPrimary: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 11,
    fontWeight: 800,
    textTransform: 'uppercase',
    letterSpacing: '0.08em',
    background: '#000',
    color: '#fff',
    border: '1px solid #000',
    padding: '8px 12px',
    cursor: 'pointer',
    width: '100%',
    minHeight: 32,
  }

  const btnSecondary: React.CSSProperties = {
    ...btnPrimary,
    background: '#fff',
    color: '#000',
  }

  const btnGhost: React.CSSProperties = {
    fontFamily: 'var(--font-mono)',
    fontSize: 10,
    fontWeight: 700,
    textTransform: 'uppercase',
    letterSpacing: '0.08em',
    background: 'transparent',
    color: '#555',
    border: 'none',
    padding: '4px 0',
    cursor: 'pointer',
    textAlign: 'left',
  }

  const dividerStyle: React.CSSProperties = {
    borderBottom: '1px solid #000',
    margin: '10px 0',
  }

  // ── Render panels ─────────────────────────────────────────────────────────

  function renderSetup() {
    return (
      <>
        {/* Header */}
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px', display: 'flex', alignItems: 'center', gap: 10 }}>
          <img src={BRAND_LOGO_SRC} alt="" style={{ width: 28, height: 28, display: 'block', flexShrink: 0, imageRendering: 'pixelated' }} />
          <div style={{ fontFamily: 'var(--font-title)', fontWeight: 900, fontSize: 16, letterSpacing: '0.15em' }}>
            ENOXIAN
          </div>
        </div>

        <div style={{ padding: '10px 12px 0' }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', fontWeight: 700, letterSpacing: '0.12em', marginBottom: 12 }}>
            LOCAL-FIRST COLLABORATION
          </div>

          {/* ── Identity section ──────────────────────────────────────────── */}
          {!isEditingIdentity && userName ? (
            /* Compact identity line when already configured */
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 14, borderBottom: '1px dashed #ccc', paddingBottom: 10 }}>
              <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 800, letterSpacing: '0.06em' }}>
                {userName}
                <span style={{ fontWeight: 400, color: '#555', marginLeft: 6 }}>&middot; {deviceLabel}</span>
              </div>
              <button style={btnGhost} onClick={() => setIsEditingIdentity(true)}>EDIT</button>
            </div>
          ) : (
            /* Input fields for first run or when editing */
            <>
              <div style={rowStyle}>
                <span style={labelStyle}>USER NAME</span>
                <input
                  style={inputStyle}
                  type="text"
                  value={userName}
                  placeholder="NAME"
                  autoFocus={!userName}
                  onChange={e => setUserName(e.target.value)}
                />
              </div>
              <div style={{ ...rowStyle, marginBottom: 10 }}>
                <span style={labelStyle}>DEVICE</span>
                <input
                  style={inputStyle}
                  type="text"
                  value={deviceLabel}
                  placeholder="DIVINING"
                  onChange={e => setDeviceLabel(e.target.value)}
                />
              </div>
              {isEditingIdentity && userName && (
                <div style={{ marginBottom: 10, textAlign: 'right' }}>
                  <button style={btnGhost} onClick={async () => {
                    await saveIdentityIfNeeded()
                    setIsEditingIdentity(false)
                  }}>SAVE</button>
                </div>
              )}
              <div style={{ ...dividerStyle, marginBottom: 12 }} />
            </>
          )}

          {error && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#000', background: '#fff', border: '1px solid #000', padding: '4px 8px', marginBottom: 8, fontWeight: 700 }}>
              {error}
            </div>
          )}

          {linkSuccess && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', marginBottom: 8, fontWeight: 700 }}>
              DEVICE LINKED &middot; IDENTITY LOADED
            </div>
          )}

          {/* Primary actions */}
          <div style={{ display: 'grid', gap: 6, marginBottom: 8 }}>
            <button style={btnPrimary} onClick={handleInitClick} disabled={loading}>
              CREATE NEW CIRCLE
            </button>
            <button style={btnSecondary} onClick={handleEnterClick} disabled={loading}>
              ENTER THE CIRCLE
            </button>
          </div>
        </div>

        {/* Link another device — only shown when no cryptographic user key yet */}
        {!hasUserKey && (
          <div style={{ borderTop: '1px solid #ddd', padding: '8px 12px' }}>
            <button style={btnGhost} onClick={() => setLinkExpanded(v => !v)}>
              {linkExpanded ? '↑' : '↓'} LINK THIS DEVICE TO AN EXISTING USER
            </button>

            {linkExpanded && (
              <div style={{ marginTop: 8 }}>
                <div style={dividerStyle} />
                <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', fontWeight: 700, marginBottom: 8, letterSpacing: '0.06em' }}>
                  ENTER YOUR NAME AND RECOVERY PHRASE FROM ANOTHER DEVICE
                </div>
                <div style={rowStyle}>
                  <span style={labelStyle}>USER NAME</span>
                  <input style={inputStyle} type="text" value={linkHandle} onChange={e => setLinkHandle(e.target.value)} />
                </div>
                <div style={{ ...rowStyle, alignItems: 'flex-start' }}>
                  <span style={{ ...labelStyle, paddingTop: 6 }}>MNEMONIC</span>
                  <textarea
                    style={{ ...inputStyle, minHeight: 60, resize: 'vertical', fontFamily: 'var(--font-mono)', fontSize: 10 } as React.CSSProperties}
                    value={linkMnemonic}
                    onChange={e => setLinkMnemonic(e.target.value)}
                    placeholder="24 WORDS"
                  />
                </div>
                <button style={{ ...btnSecondary, marginTop: 4 }} onClick={handleLinkDevice} disabled={loading}>
                  {loading ? '...' : 'LINK DEVICE'}
                </button>
              </div>
            )}
          </div>
        )}
      </>
    )
  }

  function renderInitForm() {
    return (
      <>
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px' }}>
          <div style={{ fontFamily: 'var(--font-title)', fontWeight: 900, fontSize: 14, letterSpacing: '0.1em' }}>CREATE NEW CIRCLE</div>
        </div>
        <div style={{ padding: '12px 12px 10px' }}>
          <div style={dividerStyle} />
          <div style={rowStyle}>
            <span style={labelStyle}>CIRCLE NAME</span>
            <input
              style={inputStyle}
              type="text"
              value={circleName}
              placeholder="NAME"
              onChange={e => setCircleName(e.target.value)}
              autoFocus
            />
          </div>
          <div style={{ ...rowStyle, marginBottom: 12 }}>
            <span style={labelStyle}>JOIN APPROVAL</span>
            <div style={{ display: 'flex', gap: 0 }}>
              <button
                style={{
                  ...btnSecondary,
                  width: 'auto',
                  flex: 1,
                  background: joinPolicy === 'auto' ? '#000' : '#fff',
                  color: joinPolicy === 'auto' ? '#fff' : '#000',
                }}
                onClick={() => setJoinPolicy('auto')}
              >AUTO</button>
              <button
                style={{
                  ...btnSecondary,
                  width: 'auto',
                  flex: 1,
                  marginLeft: -1,
                  background: joinPolicy === 'manual' ? '#000' : '#fff',
                  color: joinPolicy === 'manual' ? '#fff' : '#000',
                }}
                onClick={() => setJoinPolicy('manual')}
              >MANUAL</button>
            </div>
          </div>

          {error && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#000', background: '#fff', border: '1px solid #000', padding: '4px 8px', marginBottom: 8, fontWeight: 700 }}>
              {error}
            </div>
          )}

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 6 }}>
            <button style={btnPrimary} onClick={handleCreateCircle} disabled={loading}>
              {loading ? '...' : 'CREATE'}
            </button>
            <button style={btnSecondary} onClick={handleBack} disabled={loading}>BACK</button>
          </div>
        </div>
      </>
    )
  }

  function renderEnterForm() {
    return (
      <>
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px' }}>
          <div style={{ fontFamily: 'var(--font-title)', fontWeight: 900, fontSize: 14, letterSpacing: '0.1em' }}>ENTER THE CIRCLE</div>
        </div>
        <div style={{ padding: '12px 12px 10px' }}>
          <div style={dividerStyle} />
          <div style={{ ...rowStyle, alignItems: 'flex-start', marginBottom: 12 }}>
            <span style={{ ...labelStyle, paddingTop: 6 }}>INVITE LINK</span>
            <textarea
              style={{
                ...inputStyle,
                minHeight: 64,
                resize: 'vertical',
                fontFamily: 'var(--font-mono)',
                fontSize: 10,
                textTransform: 'none',
              } as React.CSSProperties}
              value={inviteUri}
              onChange={e => setInviteUri(e.target.value)}
              placeholder="PASTE URI"
              autoFocus
            />
          </div>

          {error && (
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#000', background: '#fff', border: '1px solid #000', padding: '4px 8px', marginBottom: 8, fontWeight: 700 }}>
              {error}
            </div>
          )}

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 6 }}>
            <button style={btnPrimary} onClick={handleJoinCircle} disabled={loading}>
              {loading ? '...' : 'JOIN'}
            </button>
            <button style={btnSecondary} onClick={handleBack} disabled={loading}>BACK</button>
          </div>
        </div>
      </>
    )
  }

  function renderMnemonicBackup() {
    return (
      <>
        <div style={{ borderBottom: '2px solid #000', padding: '8px 12px' }}>
          <div style={{ fontFamily: 'var(--font-title)', fontWeight: 900, fontSize: 14, letterSpacing: '0.1em' }}>SAVE YOUR RECOVERY PHRASE</div>
        </div>
        <div style={{ padding: '12px 12px 10px' }}>
          <div style={dividerStyle} />
          <p style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: '#555', fontWeight: 700, marginBottom: 10, lineHeight: 1.5 }}>
            WRITE THESE WORDS AND KEEP THEM SAFE.<br />
            YOU NEED THESE WORDS TO LINK OTHER DEVICES.
          </p>
          <div style={{
            border: '1px solid #000',
            padding: '10px 12px',
            fontFamily: 'var(--font-mono)',
            fontSize: 11,
            fontWeight: 700,
            lineHeight: 1.8,
            marginBottom: 12,
            wordBreak: 'break-word',
            background: '#fff',
            color: '#000',
            letterSpacing: '0.03em',
          }}>
            {mnemonic || '(GENERATING...)'}
          </div>
          <button style={btnPrimary} onClick={handleMnemonicConfirmed}>
            I HAVE SAVED IT
          </button>
        </div>
      </>
    )
  }

  // ── JSX ───────────────────────────────────────────────────────────────────

  return (
    <>
      {/* Three.js angel canvas — full screen, behind everything */}
      {/* Solid background to hide the app shell while it loads underneath */}
      <div id="landing-solid-bg" style={{ position: 'fixed', inset: 0, zIndex: 4999, background: '#fff', transition: 'opacity 800ms ease' }} />
      <div style={{ position: 'fixed', inset: 0, zIndex: 5000, pointerEvents: 'none' }}>
        <div ref={mountRef} className="ritual-canvas" style={{ mixBlendMode: 'multiply', width: '100%', height: '100%' }} />
        {/* We fade OUT the dither right as the eruption starts to show the clean 3D scene, then let the transition class handle the end */}
        {/* NOTE: Removing the ritual-dither class here entirely because it adds a CSS background pattern of dots which conflicts with the shader! */}
        <div style={{ opacity: isErupting ? 0 : 0.42, transition: 'opacity 0.4s ease' }} />
      </div>

      {/* UI overlay — hidden once eruption begins */}
      <div
        style={{
          position: 'fixed',
          top: '50%',
          left: 'max(10%, calc(50vw - 400px))', // Ensures it stays left but doesn't get pushed off on tiny screens
          transform: 'translateY(-50%)',
          zIndex: 5010,
          width: 420,
          maxWidth: 'calc(100vw - 24px)',
          opacity: isErupting ? 0 : 1,
          pointerEvents: isErupting ? 'none' : 'auto',
          transition: 'opacity 0.4s ease',
        }}
      >
        <div
          className="sys-window"
          style={{
            background: '#fff',
            border: '2px solid #000',
            boxShadow: '6px 6px 0 #000',
            position: 'relative',
          }}
        >
          {uiState === 'setup' && renderSetup()}
          {uiState === 'init-form' && renderInitForm()}
          {uiState === 'enter-form' && renderEnterForm()}
          {uiState === 'mnemonic-backup' && renderMnemonicBackup()}
        </div>
      </div>
    </>
  )
}
