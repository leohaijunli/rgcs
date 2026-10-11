import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'
import en from './en'

// English is the single language for now; the i18next structure keeps adding
// more locales trivial (see docs/ARCHITECTURE.md).
i18n.use(initReactI18next).init({
  resources: { en: { translation: en } },
  lng: 'en',
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
})

export default i18n