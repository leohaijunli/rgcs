// About (operator request 2026-10-18): version, copyright and the data
// attribution the plan requires (H9: the NRCan CanElevation credit travels
// with the app, not only with exported files).

import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { isTauri } from '../../inspector/mock'

export default function AboutDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { t } = useTranslation()
  const [version, setVersion] = useState<string>('dev')

  useEffect(() => {
    if (!open || !isTauri()) return
    void import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then(setVersion)
      .catch(() => setVersion('dev'))
  }, [open])

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/50" />
        <Dialog.Content
          data-testid="about-dialog"
          className="fixed left-1/2 top-24 z-50 w-[320px] -translate-x-1/2 rounded border border-line bg-panel p-3 shadow-2xl"
        >
          <div className="mb-2 flex items-center justify-between">
            <Dialog.Title className="text-sm font-medium">{t('about.title')}</Dialog.Title>
            <Dialog.Close asChild>
              <button aria-label={t('project.close')} className="text-muted hover:text-ink">
                <X size={14} />
              </button>
            </Dialog.Close>
          </div>
          <div className="space-y-1.5 text-xs text-muted" data-testid="about-body">
            <div className="text-base font-semibold tracking-wide text-accent">
              {t('app.title')} <span className="mono text-sm text-ink">v{version}</span>
            </div>
            <div>{t('about.copyright')}</div>
            <div>{t('about.license')}</div>
            <div className="border-t border-line pt-1.5">{t('about.attribution')}</div>
            <div>{t('about.attributionLicense')}</div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
