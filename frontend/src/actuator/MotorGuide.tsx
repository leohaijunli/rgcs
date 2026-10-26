import type { MotorGuide as MotorGuideState } from './benchTypes'

/** One visible next step; the matching enabled control pulses in place. */
export default function MotorGuide({ guide }: { guide: MotorGuideState }) {
  return (
    <div
      className={`motor-guide ${guide.tone}`}
      data-testid="motor-guide"
      data-action={guide.action}
      role="status"
      aria-live="polite"
    >
      <span className="section-title">Next step</span>
      <strong>{guide.title}</strong>
      <span className="motor-guide-detail">{guide.detail}</span>
    </div>
  )
}
