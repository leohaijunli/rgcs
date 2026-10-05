// Normalize an Intl-incompatible `navigator.language` (e.g. "en-US@posix" in
// headless Chromium / some WebView2 builds) before any library reads it.
// Libraries like uPlot build `Intl.NumberFormat(navigator.language)` at module
// scope; an invalid locale string throws and blanks the whole app.
const lang = typeof navigator !== 'undefined' ? navigator.language : ''

if (lang.includes('@') || lang === '' || lang === 'unknown') {
  try {
    Object.defineProperty(navigator, 'language', {
      get: () => 'en-US',
      configurable: true,
    })
  } catch {
    /* not configurable; fall back to leaving it */
  }
}