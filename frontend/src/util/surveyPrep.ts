// U9b: map a `dem-prep hrdem` failure message onto the coarse kind used by
// the "prepare survey area" card. Kept out of the component so the wording
// contract can be unit-checked without a DOM (`scripts/check-survey-prep.mjs`).

/** Failure kinds; each has `survey.prep.fail.<kind>.{title,next}` copy. */
export type PrepFailKind =
  | 'offline'
  | 'uncovered'
  | 'crossProject'
  | 'oversized'
  | 'gdal'
  | 'generic'

/** Classify a backend error string. An unrecognised message degrades to
 * `generic` so the card always states *something* and never goes blank. */
export function classifyPrepError(message: string): PrepFailKind {
  const m = message.toLowerCase()
  if (m.includes('offline') || m.includes('stac unreachable')) return 'offline'
  if (m.includes('crosses project')) return 'crossProject'
  if (m.includes('no hrdem-lidar project covers')) return 'uncovered'
  if (m.includes('exceeds') && m.includes('limit')) return 'oversized'
  // Only a *missing* tool is the "GDAL not found" kind; a runtime failure of
  // an installed tool ("gdalwarp failed: …") must degrade to `generic` so the
  // card does not tell the operator to install GDAL they already have.
  if (m.includes('not found on path') || (m.includes('not found') && m.includes('gdal')))
    return 'gdal'
  return 'generic'
}
