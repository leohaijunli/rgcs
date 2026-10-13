// Cesium viewer bootstrap: WebGL guard, offline grid + selectable imagery
// (OSM streets / Esri satellite / OpenTopoMap), initial camera. Extracted
// from `MapView.tsx` (issues.md #29).

import * as Cesium from 'cesium'
import { cssVar } from '../design-system/theme'
import { HOME_LAT, HOME_LON } from './constants'

/** UI4 base-imagery de-noising: pull the saturation down a touch and lift
 * contrast so waypoints, flight lines and overlays stay legible on top of
 * busy satellite/OSM tiles. Applied to every imagery layer swap. */
const BASE_SATURATION = 0.8
const BASE_BRIGHTNESS = 0.95
const BASE_CONTRAST = 1.05

/** UI4: the hillshade blends into the base imagery instead of hiding it —
 * the dem-prep PNG is opaque grayscale, so at full alpha it replaced the
 * satellite/OSM tiles entirely. */
const OVERLAY_ALPHA = 0.55

/** Apply the de-noise tuning to an imagery layer (UI4). */
function toneDown(layer: Cesium.ImageryLayer): Cesium.ImageryLayer {
  layer.saturation = BASE_SATURATION
  layer.brightness = BASE_BRIGHTNESS
  layer.contrast = BASE_CONTRAST
  return layer
}

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
  toneDown(viewer.imageryLayers.addImageryProvider(imageryProvider('google-satellite'), 1))
  viewer.camera.setView({
    destination: Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, 12000),
  })
  return viewer
}

/**
 * The imagery providers (operator request 2026-10-13: Google Maps Platform
 * only — OSM/Esri/OpenTopo were removed). Two Google layers: satellite
 * (lyrs=s) and hybrid — satellite imagery with street labels (lyrs=y) —
 * both WGS84-aligned like the waypoints; unreachable from mainland China
 * without a proxy. Swapping to the official Map Tiles API is a URL + key
 * change (tile.googleapis.com/v1/2dtiles/...).
 */
function imageryProvider(style: 'google-satellite' | 'google-hybrid'): Cesium.ImageryProvider {
  const lyrs = style === 'google-hybrid' ? 'y' : 's'
  return new Cesium.UrlTemplateImageryProvider({
    url: `https://mt{s}.google.com/vt/lyrs=${lyrs}&x={x}&y={y}&z={z}`,
    subdomains: ['0', '1', '2', '3'],
    maximumLevel: 20,
    credit: 'Google',
  })
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

/**
 * Swap the imagery layer's provider (layer 1 keeps its stack position above
 * the offline grid; `imageryProvider` itself is read-only, so the layer is
 * replaced).
 */
export function setImageryStyle(
  viewer: Cesium.Viewer,
  style: 'google-satellite' | 'google-hybrid',
): void {
  const layers = viewer.imageryLayers
  if (layers.length < 2) return
  layers.remove(layers.get(1))
  toneDown(layers.addImageryProvider(imageryProvider(style), 1))
}

/**
 * Add or replace the terrain hillshade overlay (plan A): a dem-prep
 * hillshade PNG draped by its geographic bounds, above the imagery layer.
 * `null` removes it.
 *
 * Provider loading is async, so every call bumps a per-viewer generation;
 * a load that finishes after a newer call (or a clear) is dropped instead
 * of re-adding a stale overlay (M2).
 */
const overlayGeneration = new WeakMap<Cesium.Viewer, number>()

export function setTerrainOverlay(
  viewer: Cesium.Viewer,
  overlay: { url: string | null; bounds: { north: number; south: number; east: number; west: number } } | null,
): void {
  const layers = viewer.imageryLayers
  // Remove any previous overlay (layers above index 1 are ours).
  while (layers.length > 2) {
    layers.remove(layers.get(layers.length - 1))
  }
  const generation = (overlayGeneration.get(viewer) ?? 0) + 1
  overlayGeneration.set(viewer, generation)
  // A tile without a hillshade (raster products) clears the shade layer but
  // keeps the DEM loaded — drape / terrain mode do not depend on the PNG.
  if (!overlay?.url) return
  const rectangle = Cesium.Rectangle.fromDegrees(
    overlay.bounds.west,
    overlay.bounds.south,
    overlay.bounds.east,
    overlay.bounds.north,
  )
  void Cesium.SingleTileImageryProvider.fromUrl(overlay.url, { rectangle }).then(
    (provider) => {
      // The load raced a newer call or a clear: that call owns the stack now.
      if (overlayGeneration.get(viewer) !== generation || viewer.isDestroyed()) return
      const layer = layers.addImageryProvider(provider, 2)
      layer.alpha = OVERLAY_ALPHA
    },
    () => {
      /* a broken PNG is not fatal: the imagery stays */
    },
  )
}

/**
 * Switch the globe's terrain provider (plan B): `url` is the tileset
 * **base URL** served by the app (`terrain3d_start`, trailing slash —
 * `fromUrl` appends `layer.json` itself), `null` resets to the ellipsoid.
 *
 * `fromUrl` fetches `layer.json` before the provider exists, so the swap
 * is async — a per-viewer generation drops a slow load that raced a newer
 * switch (same guard as [`setTerrainOverlay`]). Rejects when the endpoint
 * is unreachable or its manifest is invalid; the caller surfaces that.
 */
const terrain3dGeneration = new WeakMap<Cesium.Viewer, number>()

export async function setTerrain3d(viewer: Cesium.Viewer, url: string | null): Promise<void> {
  const generation = (terrain3dGeneration.get(viewer) ?? 0) + 1
  terrain3dGeneration.set(viewer, generation)
  if (!url) {
    if (!viewer.isDestroyed()) viewer.terrainProvider = new Cesium.EllipsoidTerrainProvider()
    return
  }
  const provider = await Cesium.CesiumTerrainProvider.fromUrl(url, {
    // Negotiates the octvertexnormals extension (layer.json always ships
    // normals; requestVertexNormals also enables globe lighting from them).
    requestVertexNormals: true,
  })
  if (terrain3dGeneration.get(viewer) !== generation || viewer.isDestroyed()) return
  viewer.terrainProvider = provider
}
