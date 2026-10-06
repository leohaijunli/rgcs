// Normalize an Intl-incompatible `navigator.language` before any library reads
// it. Libraries like uPlot build `Intl.NumberFormat(navigator.language)` at
// module scope; an invalid locale throws a RangeError and blanks the whole app.
//
// Seen in the wild: "en-US@posix" (headless Chromium / WebView2) and the POSIX
// locale "C" (WebKitGTK on Linux reports navigator.language === "C", which is
// not a valid BCP-47 tag and makes Intl throw).
const lang = typeof navigator !== 'undefined' ? navigator.language : ''

function isValidLanguageTag(tag: string): boolean {
  if (!tag || tag.includes('@')) return false
  try {
    Intl.getCanonicalLocales(tag)
    return true
  } catch {
    return false
  }
}

if (!isValidLanguageTag(lang)) {
  const define = (target: object): void => {
    try {
      Object.defineProperty(target, 'language', {
        get: () => 'en-US',
        configurable: true,
      })
    } catch {
      /* not configurable; try the next target */
    }
  }
  // The instance own-property shadows the prototype getter; patching the
  // prototype as well covers engines where the instance is read-only.
  if (typeof navigator !== 'undefined') define(navigator)
  if (typeof Navigator !== 'undefined') define(Navigator.prototype)
  try {
    Object.defineProperty(navigator, 'languages', {
      get: () => ['en-US'],
      configurable: true,
    })
  } catch {
    /* leave as-is */
  }
}
