import { LogOut, PauseCircle, PlayCircle } from 'lucide-react'

interface Props {
  disabled: boolean
  onToggle: () => void
  onLeave: () => void
}

export default function CircleMembership({ disabled, onToggle, onLeave }: Props) {
  return (
    <div className="circle-membership">
      <section className="membership-action" aria-labelledby="membership-participation-title">
        <div className="membership-action__icon" aria-hidden="true">{disabled ? <PlayCircle size={20} /> : <PauseCircle size={20} />}</div>
        <div className="membership-action__copy">
          <div className="membership-action__heading">
            <h3 id="membership-participation-title">Participation</h3>
            <span className={`membership-status${disabled ? ' is-paused' : ''}`}>{disabled ? 'Disabled' : 'Enabled'}</span>
          </div>
          <p>{disabled ? 'Participation is paused on this device. Enable to resume.' : 'Pause this Circle on this device. You can enable it again anytime.'}</p>
        </div>
        <button type="button" className="membership-action__button" onClick={onToggle}>{disabled ? 'Enable Circle' : 'Disable Circle'}</button>
      </section>
      <section className="membership-action membership-action--leave" aria-labelledby="membership-leave-title">
        <div className="membership-action__icon" aria-hidden="true"><LogOut size={20} /></div>
        <div className="membership-action__copy">
          <h3 id="membership-leave-title">Leave this Circle</h3>
          <p>Remove this Circle’s configuration from this device. Your workspace files are kept.</p>
          <span className="membership-action__hint">You’ll be asked to confirm before leaving.</span>
        </div>
        <button type="button" className="membership-action__button membership-action__button--leave" onClick={onLeave}>Leave Circle…</button>
      </section>
    </div>
  )
}
