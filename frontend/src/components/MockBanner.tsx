import { useTranslation } from 'react-i18next'
import { FlaskConical } from 'lucide-react'
import { useTelemetryStore } from '../stores/telemetry'

/**
 * Prominent indicator shown while the browser mock feed is the telemetry
 * source, so simulated data is never mistaken for a live link (issue #27).
 */
export default function MockBanner() {
  const { t } = useTranslation()
  const isMock = useTelemetryStore((s) => s.isMock)
  if (!isMock) return null

  return (
    <div
      role="status"
      className="pointer-events-none absolute left-1/2 top-2 z-20 flex -translate-x-1/2 items-center gap-2 rounded border border-warn bg-warn/15 px-3 py-1 text-xs font-medium text-warn shadow-xl backdrop-blur"
    >
      <FlaskConical size={14} />
      <span className="tracking-wide">{t('mock.banner')}</span>
      <span className="font-normal text-muted">{t('mock.detail')}</span>
    </div>
  )
}
