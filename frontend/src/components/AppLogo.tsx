import { useTranslation } from 'react-i18next'
import './branding.css'

export type AppLogoVariant = 'full' | 'wordmark' | 'mark'

const VIEW_BOXES: Record<AppLogoVariant, string> = {
  full: '210 28 260 128',
  wordmark: '216 34 248 92',
  mark: '0 0 96 96',
}

/** The supplied Q-Mag-logo.svg lettering, with a transparent, theme-aware
 * surface. The compact mark uses the same Q glyph in a square viewport. */
export default function AppLogo({
  variant = 'full',
  className = '',
}: {
  variant?: AppLogoVariant
  className?: string
}) {
  const { t } = useTranslation()
  return (
    <svg
      className={`app-logo app-logo-${variant} ${className}`}
      viewBox={VIEW_BOXES[variant]}
      role="img"
      aria-label={t('app.title')}
      focusable="false"
      data-testid="app-logo"
      data-logo-variant={variant}
    >
      <text
        x={variant === 'mark' ? 48 : 340}
        y={variant === 'mark' ? 73 : 105}
        textAnchor="middle"
        fontFamily="Helvetica, Arial, sans-serif"
        fontSize="76"
        fontWeight="bold"
        fill="currentColor"
      >
        {variant === 'mark' ? 'Q' : <>Q<tspan className="app-logo-accent">-</tspan>Mag</>}
      </text>
      {variant === 'full' && (
        <text
          className="app-logo-subtitle"
          x="340"
          y="145"
          textAnchor="middle"
          fontFamily="Helvetica, Arial, sans-serif"
          fontSize="14"
          letterSpacing="6"
        >
          {t('app.tagline')}
        </text>
      )}
    </svg>
  )
}
