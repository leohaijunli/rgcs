import * as Popover from '@radix-ui/react-popover'
import { ChevronDown, ChevronLeft, ChevronRight } from 'lucide-react'
import { useRef, useState, type KeyboardEvent } from 'react'
import { useTranslation } from 'react-i18next'
import type { View } from '../stores/ui'

const STAGES: readonly View[] = ['prepare', 'plan', 'preflight', 'fly', 'review']

export interface WorkflowNavProps {
  view: View
  badge: string | null
  onChange: (view: View) => void
}

/** Compact wizard navigation. The numbers identify stages, not completion;
 * selecting a stage only changes the workspace view. */
export default function WorkflowNav({ view, badge, onChange }: WorkflowNavProps) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const buttons = useRef<Array<HTMLButtonElement | null>>([])
  const firstFocus = useRef(0)
  const current = STAGES.indexOf(view)
  const currentLabel = t(`nav.${view}`)

  const select = (stage: View) => {
    onChange(stage)
    setOpen(false)
  }

  const moveFocus = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let next = index
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowRight':
        next = (index + 1) % STAGES.length
        break
      case 'ArrowUp':
      case 'ArrowLeft':
        next = (index + STAGES.length - 1) % STAGES.length
        break
      case 'Home':
        next = 0
        break
      case 'End':
        next = STAGES.length - 1
        break
      default:
        return
    }
    event.preventDefault()
    buttons.current[next]?.focus()
  }

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="workflow-trigger"
          data-testid="workflow-open"
          data-stage={view}
          aria-label={t('workflow.currentStage', {
            stage: currentLabel,
            step: current + 1,
            total: STAGES.length,
          })}
          onPointerDown={() => { firstFocus.current = current }}
          onKeyDown={(event) => {
            firstFocus.current = current
            if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
              event.preventDefault()
              setOpen(true)
            }
          }}
        >
          <span className="workflow-progress" aria-hidden="true">{current + 1}/{STAGES.length}</span>
          <span className="workflow-current-label">{currentLabel}</span>
          {view === 'preflight' && badge !== null && <span className="workflow-badge">{badge}</span>}
          <ChevronDown size={13} aria-hidden="true" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className="workflow-panel"
          data-testid="workflow-panel"
          aria-label={t('workflow.title')}
          align="start"
          sideOffset={8}
          collisionPadding={12}
          onOpenAutoFocus={(event) => {
            event.preventDefault()
            buttons.current[firstFocus.current]?.focus()
          }}
        >
          <div className="workflow-heading">
            <strong>{t('workflow.title')}</strong>
            <span>{current + 1} / {STAGES.length}</span>
          </div>
          <ol className="workflow-steps">
            {STAGES.map((stage, index) => (
              <li key={stage}>
                <button
                  type="button"
                  className={`workflow-step${stage === view ? ' current' : ''}`}
                  data-testid={`workflow-stage-${stage}`}
                  aria-label={t(`nav.${stage}`)}
                  aria-current={stage === view ? 'step' : undefined}
                  ref={(element) => { buttons.current[index] = element }}
                  onKeyDown={(event) => moveFocus(event, index)}
                  onClick={() => select(stage)}
                >
                  <span className="workflow-step-number" aria-hidden="true">{index + 1}</span>
                  <span className="workflow-step-copy">
                    <span className="workflow-step-label">{t(`nav.${stage}`)}</span>
                    <span className="workflow-step-description">{t(`workflow.descriptions.${stage}`)}</span>
                  </span>
                  {stage === 'preflight' && badge !== null && <span className="workflow-badge">{badge}</span>}
                </button>
              </li>
            ))}
          </ol>
          <div className="workflow-actions">
            <button
              type="button"
              data-testid="workflow-previous"
              disabled={current === 0}
              onClick={() => select(STAGES[current - 1])}
            >
              <ChevronLeft size={14} aria-hidden="true" />
              {t('workflow.previous')}
            </button>
            <button
              type="button"
              data-testid="workflow-next"
              disabled={current === STAGES.length - 1}
              onClick={() => select(STAGES[current + 1])}
            >
              {t('workflow.next')}
              <ChevronRight size={14} aria-hidden="true" />
            </button>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  )
}
