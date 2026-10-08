// Single ground-pick helper for the map (finding 16, ADR-013).
//
// `camera.pickEllipsoid` is correct on a bare globe but returns the wrong
// ground point once a DEM is loaded, because the terrain is above the
// ellipsoid. `globe.pick` intersects the actual terrain, so use it whenever a
// real terrain provider is present and fall back to the ellipsoid otherwise
// (the current state: `createViewer` installs the default ellipsoid terrain).
// Returning `null` for the sky lets callers skip the update instead of
// inventing a ground point.

import * as Cesium from 'cesium'

/** A ground point under a screen position, in degrees (WGS84). */
export interface PickedGround {
  lat: number
  lon: number
  /** Ground elevation in metres AMSL when the globe provided one, else null. */
  heightM: number | null
}

/** True when the scene is rendering real terrain above the ellipsoid. */
function hasTerrain(scene: Cesium.Scene): boolean {
  const provider = scene.globe.terrainProvider
  return (
    scene.mode === Cesium.SceneMode.SCENE3D &&
    provider instanceof Cesium.TerrainProvider &&
    !(provider instanceof Cesium.EllipsoidTerrainProvider)
  )
}

/** Pick the ground point under `position`, or `null` if it points at the sky. */
export function pickLatLon(viewer: Cesium.Viewer, position: Cesium.Cartesian2): PickedGround | null {
  const scene = viewer.scene
  let cartesian: Cesium.Cartesian3 | undefined
  if (hasTerrain(scene)) {
    const ray = viewer.camera.getPickRay(position)
    cartesian = ray ? scene.globe.pick(ray, scene) : undefined
  } else {
    cartesian = viewer.camera.pickEllipsoid(position, scene.globe.ellipsoid)
  }
  if (!cartesian) return null
  const carto = Cesium.Cartographic.fromCartesian(cartesian, scene.globe.ellipsoid)
  if (!carto) return null
  return {
    lat: Cesium.Math.toDegrees(carto.latitude),
    lon: Cesium.Math.toDegrees(carto.longitude),
    heightM: Number.isFinite(carto.height) ? carto.height : null,
  }
}
