/** A small, deterministic seal: stable across messages, devices and sessions. */
export default function AgentAvatar({ identity }: { identity: string }) {
  let hash = 2166136261
  for (const character of identity.toLowerCase().trim()) {
    hash = Math.imul(hash ^ character.charCodeAt(0), 16777619) >>> 0
  }
  const family = hash % 3
  const turn = ((hash >>> 4) % 4) * 45
  return (
    <span className="agent-avatar" aria-hidden="true">
      <svg viewBox="0 0 32 32" fill="none">
        <path className="agent-avatar__corners" d="M2 8V2h6m16 0h6v6M2 24v6h6m16 0h6v-6" />
        <g transform={`rotate(${turn} 16 16)`}>
          {family === 0 && <>
            <path d="m16 5 11 11-11 11L5 16Z" />
            <path d="M10 10h12v12H10Z" />
            <path d="M16 5v5m11 6h-5m-6 11v-5M5 16h5" />
          </>}
          {family === 1 && <>
            <circle cx="16" cy="16" r="10" />
            <path d="M12 7v18m8-18v18M7 12h18M7 20h18" />
            <path d="m16 10 6 6-6 6-6-6Z" />
          </>}
          {family === 2 && <>
            <path d="m16 5 9 5v12l-9 5-9-5V10Z" />
            <path d="m16 9 6 7-6 7-6-7Zm-9 1 9 6 9-6M16 16v11" />
          </>}
        </g>
        <rect x="14" y="14" width="4" height="4" fill="currentColor" stroke="none" />
        {[0, 1, 2].map(index => <rect key={index} x={11 + index * 4} y="30" width="1" height="1" fill="currentColor" stroke="none" opacity={(hash >>> (index + 8)) & 1 ? 0.8 : 0.2} />)}
      </svg>
    </span>
  )
}
