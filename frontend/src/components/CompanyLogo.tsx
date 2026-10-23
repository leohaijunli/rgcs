import { useTranslation } from 'react-i18next'
import './branding.css'

/** Both supplied company assets share a neutral surface so their original
 * dark lettering and blue emblem remain readable in every application theme. */
export default function CompanyLogo({ className = '' }: { className?: string }) {
  const { t } = useTranslation()
  const base = `${import.meta.env.BASE_URL}brand/`
  return (
    <span className={`company-logo ${className}`}>
      <img
        className="company-logo-dark"
        src={`${base}company-dark.png`}
        alt={t('app.company')}
        data-testid="company-logo-dark"
      />
      <img
        className="company-logo-light"
        src={`${base}company-light.jpg`}
        alt={t('app.company')}
        data-testid="company-logo-light"
      />
    </span>
  )
}
