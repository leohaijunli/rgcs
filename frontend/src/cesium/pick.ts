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

/**
 * Intersect the pick ray with the surface at `heightM` above the ellipsoid
 * (an inflated ellipsoid; the error against a true constant-height surface is
 * second order). Returns `undefined` when the ray misses it or the camera is
 * at/below that height, where the intersection would be meaningless.
 */
function pickAtHeight(
  viewer: Cesium.Viewer,
  position: Cesium.Cartesian2,
  heightM: number,
): Cesium.Cartesian3 | undefined {
  const scene = viewer.scene
  if (scene.mode !== Cesium.SceneMode.SCENE3D) return undefined
  if (viewer.camera.positionCartographic.height <= heightM + 1) return undefined
  const ray = viewer.camera.getPickRay(position)
  if (!ray) return undefined
  const r = scene.globe.ellipsoid.radii
  const shell = new Cesium.Ellipsoid(r.x + heightM, r.y + heightM, r.z + heightM)
  const hit = Cesium.IntersectionTests.rayEllipsoid(ray, shell)
  if (!hit || hit.start <= 0) return undefined
  return Cesium.Ray.getPoint(ray, hit.start, new Cesium.Cartesian3())
}

/**
 * Pick the ground point under `position`, or `null` if it points at the sky.
 *
 * With `heightM` (render height of the thing being placed or dragged) the ray
 * is intersected with the horizontal surface at that height instead of the
 * terrain, so a waypoint drawn at 100 m stays under the cursor from a tilted
 * camera rather than sliding to wherever the ground behind it is. Falls back
 * to the ground pick when that surface cannot be hit.
 */
export function pickLatLon(
  viewer: Cesium.Viewer,
  position: Cesium.Cartesian2,
  heightM?: number | null,
): PickedGround | null {
  const scene = viewer.scene
  let cartesian: Cesium.Cartesian3 | undefined
  if (heightM != null && Number.isFinite(heightM)) {
    cartesian = pickAtHeight(viewer, position, heightM)
    if (cartesian) {
      const carto = Cesium.Cartographic.fromCartesian(cartesian, scene.globe.ellipsoid)
      if (carto) {
        return {
          lat: Cesium.Math.toDegrees(carto.latitude),
          lon: Cesium.Math.toDegrees(carto.longitude),
          heightM,
        }
      }
    }
  }
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
