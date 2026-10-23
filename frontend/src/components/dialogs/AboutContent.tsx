// Settings → About: one runtime version and complete application/data credits.
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { isTauri } from '../../inspector/mock'
import AppLogo from '../AppLogo'
import CompanyLogo from '../CompanyLogo'

export default function AboutContent() {
  const { t } = useTranslation()
  const [version, setVersion] = useState<string>('dev')

  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    void import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((value) => { if (!cancelled) setVersion(value) })
      .catch(() => { if (!cancelled) setVersion('dev') })
    return () => { cancelled = true }
  }, [])

  return (
    <section className="space-y-3 text-sm text-muted" data-testid="about-body" aria-label={t('about.title')}>
      <h2 className="sr-only">{t('app.title')}</h2>
      <div className="flex flex-wrap items-center gap-3">
        <AppLogo className="about-app-logo" />
        <span className="mono text-sm text-ink" data-testid="app-version">v{version}</span>
      </div>
      <div>{t('about.copyright')}</div>
      <div>{t('about.license')}</div>
      <div className="border-t border-line pt-3">
        <CompanyLogo />
      </div>
      <div className="space-y-2 border-t border-line pt-3">
        <div>{t('about.attribution')}</div>
        <div>{t('about.attributionLicense')}</div>
      </div>
    </section>
  )
}
