// Cesium viewer bootstrap: WebGL guard, offline grid + OSM imagery, initial
// camera. Extracted from `MapView.tsx` (issues.md #29).

import * as Cesium from 'cesium'
import { cssVar } from '../design-system/theme'
import { HOME_LAT, HOME_LON } from './constants'

/** Initial camera / HOME position in degrees. */
interface LatLonLike {
  lat: number
  lon: number
}

/** Fail loudly instead of showing a black map if the webview has no WebGL. */
export function assertWebGl(): void {
  const probe = document.createElement('canvas')
  if (!probe.getContext('webgl2') && !probe.getContext('webgl')) {
    throw new Error('WebGL is unavailable in this webview; the map cannot render.')
  }
}

/**
 * Create the `Viewer` with a chromeless UI, an offline grid base layer and OSM
 * imagery stacked on top (real map when online, grid fallback when offline),
 * then frame the home area. Throws if WebGL is unavailable.
 */
export function createViewer(
  container: HTMLElement,
  initial: LatLonLike = { lat: HOME_LAT, lon: HOME_LON },
): Cesium.Viewer {
  assertWebGl()

  const grid = new Cesium.GridImageryProvider({
    cells: 16,
    color: Cesium.Color.fromCssColorString(cssVar('--mg-grid-line')),
    glowColor: Cesium.Color.fromCssColorString(cssVar('--mg-grid-glow')),
    backgroundColor: Cesium.Color.fromCssColorString(cssVar('--mg-grid-bg')),
    canvasSize: 256,
  })
  const osm = new Cesium.OpenStreetMapImageryProvider({ url: 'https://tile.openstreetmap.org/' })
  const viewer = new Cesium.Viewer(container, {
    baseLayer: new Cesium.ImageryLayer(grid),
    animation: false,
    timeline: false,
    fullscreenButton: false,
    homeButton: false,
    sceneModePicker: false,
    baseLayerPicker: false,
    geocoder: false,
    navigationHelpButton: false,
    infoBox: false,
    selectionIndicator: false,
  })
  viewer.imageryLayers.addImageryProvider(osm, 1)
  viewer.camera.setView({
    destination: Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, 12000),
  })
  return viewer
}
