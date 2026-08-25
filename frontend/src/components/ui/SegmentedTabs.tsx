import type { ReactNode } from 'react'

export interface SegmentedTabOption<T extends string> {
  value: T
  content: ReactNode
  ariaLabel?: string
  title?: string
  className?: string
}

interface Props<T extends string> {
  value: T
  onChange: (value: T) => void
  options: readonly SegmentedTabOption<T>[]
  ariaLabel: string
  className: string
  tabClassName?: string
  orientation?: 'horizontal' | 'vertical'
}

export default function SegmentedTabs<T extends string>({
  value,
  onChange,
  options,
  ariaLabel,
  className,
  tabClassName = '',
  orientation = 'horizontal',
}: Props<T>) {
  return (
    <div
      className={className}
      role="tablist"
      aria-label={ariaLabel}
      aria-orientation={orientation}
    >
      {options.map(option => {
        const selected = value === option.value
        const classes = [tabClassName, option.className, selected ? 'is-active' : '']
          .filter(Boolean)
          .join(' ')

        return (
          <button
            key={option.value}
            type="button"
            role="tab"
            aria-selected={selected}
            aria-label={option.ariaLabel}
            title={option.title}
            className={classes}
            onClick={() => { if (!selected) onChange(option.value) }}
          >
            {option.content}
          </button>
        )
      })}
    </div>
  )
}
