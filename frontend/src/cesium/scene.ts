// Cesium viewer bootstrap: WebGL guard, offline grid + selectable imagery
// (OSM streets / Esri satellite / OpenTopoMap), initial camera. Extracted
// from `MapView.tsx` (issues.md #29).

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
  viewer.imageryLayers.addImageryProvider(imageryProvider('osm'), 1)
  viewer.camera.setView({
    destination: Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, 12000),
  })
  return viewer
}

/** The imagery providers (operator request: satellite + topo in addition to
 * the street map; no grid-only mode — imagery is always shown). */
function imageryProvider(style: 'osm' | 'satellite' | 'google' | 'topo'): Cesium.ImageryProvider {
  switch (style) {
    case 'google':
      // Google satellite tiles (lyrs=s): WGS84-aligned like the waypoints.
      // Unreachable from mainland China without a proxy.
      return new Cesium.UrlTemplateImageryProvider({
        url: 'https://mt{s}.google.com/vt/lyrs=s&x={x}&y={y}&z={z}',
        subdomains: ['0', '1', '2', '3'],
        maximumLevel: 20,
        credit: 'Google',
      })
    case 'satellite':
      // Esri World Imagery: global aerial coverage, the layer a survey plan
      // is actually drawn against.
      return new Cesium.UrlTemplateImageryProvider({
        url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
        maximumLevel: 19,
        credit: 'Esri, Maxar, Earthstar Geographics',
      })
    case 'topo':
      return new Cesium.UrlTemplateImageryProvider({
        url: 'https://tile.opentopomap.org/{z}/{x}/{y}.png',
        maximumLevel: 17,
        credit: 'OpenTopoMap (CC-BY-SA)',
      })
    default:
      return new Cesium.OpenStreetMapImageryProvider({ url: 'https://tile.openstreetmap.org/' })
  }
}

/**
 * Swap the imagery layer's provider (layer 1 keeps its stack position above
 * the offline grid; `imageryProvider` itself is read-only, so the layer is
 * replaced).
 */
export function setImageryStyle(
  viewer: Cesium.Viewer,
  style: 'osm' | 'satellite' | 'google' | 'topo',
): void {
  const layers = viewer.imageryLayers
  if (layers.length < 2) return
  layers.remove(layers.get(1))
  layers.addImageryProvider(imageryProvider(style), 1)
}

/**
 * Show or hide the two base layers created by [`createViewer`].
 *
 * Layer 0 is the offline graticule, layer 1 the imagery on top of it; the
 * Layers panel toggles the grid (imagery is always shown — no grid-only
 * mode, operator request 2026-10-10).
 */
export function applyLayerVisibility(viewer: Cesium.Viewer, showGrid: boolean): void {
  const grid = viewer.imageryLayers.get(0)
  if (grid) grid.show = showGrid
}
