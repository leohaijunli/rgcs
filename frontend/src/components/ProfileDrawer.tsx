// Bottom drawer under the map (operator request 2026-10-17): the live
// altitude profile collapses into this drawer and expands on the handle.
// The chart always follows the flight track; the drawer only controls
// visibility/height.

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import LiveAltitudeProfile from './LiveAltitudeProfile'

export default function ProfileDrawer({ defaultOpen = false }: { defaultOpen?: boolean } = {}) {
  const { t } = useTranslation()
  // U15: with the map detached the main window opens this drawer by default —
  // the freed space carries the altitude profile instead of sitting empty.
  const [open, setOpen] = useState(defaultOpen)
  useEffect(() => {
    if (defaultOpen) setOpen(true)
  }, [defaultOpen])

  return (
    <div className="border-t border-line bg-panel">
      {/* Collapsed state: a thin handle bar. */}
      {!open && (
        <button
          onClick={() => setOpen(true)}
          className="flex w-full items-center justify-center gap-2 px-2 py-1 text-[11px] text-muted hover:bg-canvas hover:text-ink"
          title={t('drawer.profile.open')}
        >
          <span>▴</span>
          {t('drawer.profile.title')}
          <span>▴</span>
        </button>
      )}
      {open && (
        <div className="flex flex-col">
          <div className="flex items-center justify-between border-b border-line px-2 py-0.5">
            <span className="text-[11px] text-muted">{t('drawer.profile.title')}</span>
            <button
              onClick={() => setOpen(false)}
              title={t('drawer.profile.close')}
              className="px-1 text-[11px] text-muted hover:text-ink"
            >
              ▾
            </button>
          </div>
          <div className="px-2 py-1">
            <LiveAltitudeProfile />
          </div>
        </div>
      )}
    </div>
  )
}