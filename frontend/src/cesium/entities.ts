// Cesium entity/layer construction: drone, trail, forward projection, home and
// the mission waypoint layer. Kept free of React so it can be unit-reasoned and
// reused; `MapView` only wires it up (issues.md #29).

import * as Cesium from 'cesium'
import { cssVar } from '../design-system/theme'
import i18n from '../i18n'
import { degFromMavInt } from '../stores/mission'
import type { MissionItem } from '../generated-types/MissionItem'
import { HOME_LAT, HOME_LON, UAV_MODEL_URI } from './constants'

/** Initial position (degrees) for the placeholder fix and the HOME marker. */
interface LatLonLike {
  lat: number
  lon: number
}

/** The fixed drone/trail/predict/home layer, created once per viewer. */
export interface DroneLayer {
  drone: Cesium.Entity
  trail: Cesium.Entity
  predict: Cesium.Entity
  home: Cesium.Entity
  /** Created once and updated in place; swapping properties rebuilds the model. */
  dronePosition: Cesium.ConstantPositionProperty
  droneOrientation: Cesium.ConstantProperty
}

/**
 * Add the static layers. The drone is hidden until the first fix: the map has
 * no terrain provider, so an untagged placeholder would sit in the air above
 * the ellipsoid ground.
 */
export function createDroneLayer(
  viewer: Cesium.Viewer,
  trailPos: () => Cesium.Cartesian3[],
  predictPos: () => Cesium.Cartesian3[],
  initial: LatLonLike = { lat: HOME_LAT, lon: HOME_LON },
): DroneLayer {
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const ok = Cesium.Color.fromCssColorString(cssVar('--mg-ok'))
  const warn = Cesium.Color.fromCssColorString(cssVar('--mg-warn'))

  const dronePosition = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, 0),
  )
  const droneOrientation = new Cesium.ConstantProperty(Cesium.Quaternion.IDENTITY)
  const drone = viewer.entities.add({
    show: false,
    position: dronePosition,
    orientation: droneOrientation,
    model: {
      uri: UAV_MODEL_URI,
      minimumPixelSize: 44,
      maximumScale: 10,
      // Keep the airframe's own materials; the silhouette is what keeps it
      // legible against the imagery at any zoom.
      silhouetteColor: accent,
      silhouetteSize: 2,
    },
    label: {
      text: i18n.t('map.uav'),
      font: '12px sans-serif',
      pixelOffset: new Cesium.Cartesian2(0, -34),
      fillColor: Cesium.Color.WHITE,
    },
  })
  const trail = viewer.entities.add({
    polyline: {
      positions: new Cesium.CallbackProperty(trailPos, false),
      width: 2,
      material: new Cesium.PolylineGlowMaterialProperty({ glowPower: 0.18, color: accent }),
    },
  })
  // Forward-projection hint: dashed and warn-tinted so it reads as a
  // prediction, not the recorded track.
  const predict = viewer.entities.add({
    polyline: {
      positions: new Cesium.CallbackProperty(predictPos, false),
      width: 2,
      material: new Cesium.PolylineDashMaterialProperty({ color: warn, dashLength: 16 }),
    },
  })
  const home = viewer.entities.add({
    position: Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, 0),
    ellipse: {
      semiMajorAxis: 12,
      semiMinorAxis: 12,
      material: ok.withAlpha(0.35),
      outline: true,
      outlineColor: ok,
    },
    label: {
      text: 'HOME',
      font: '11px sans-serif',
      pixelOffset: new Cesium.Cartesian2(0, -16),
      fillColor: Cesium.Color.WHITE,
    },
  })
  return { drone, trail, predict, home, dronePosition, droneOrientation }
}

/** Rendered mission waypoints: the connecting polyline and one point per item. */
export interface WaypointLayer {
  line: Cesium.Entity | null
  points: Map<number, Cesium.Entity>
}

/** Remove a previously rendered waypoint layer from the viewer. */
export function disposeWaypointLayer(viewer: Cesium.Viewer, layer: WaypointLayer | null): void {
  if (!layer) return
  layer.points.forEach((e) => viewer.entities.remove(e))
  layer.points.clear()
  if (layer.line) viewer.entities.remove(layer.line)
}

/** Draw the mission items as a glowing polyline plus numbered, selectable points. */
export function renderWaypoints(
  viewer: Cesium.Viewer,
  items: MissionItem[],
  selectedSeq: number | null,
): WaypointLayer {
  const layer: WaypointLayer = { line: null, points: new Map() }
  if (items.length === 0) return layer

  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const ok = Cesium.Color.fromCssColorString(cssVar('--mg-ok'))

  layer.line = viewer.entities.add({
    polyline: {
      positions: items.map((it) =>
        Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z + 5),
      ),
      width: 2,
      material: new Cesium.PolylineGlowMaterialProperty({ glowPower: 0.18, color: accent.withAlpha(0.8) }),
    },
  })

  items.forEach((it) => {
    const isSel = it.seq === selectedSeq
    const entity = viewer.entities.add({
      id: `wp-${it.seq}`,
      position: Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z + 5),
      point: {
        pixelSize: isSel ? 14 : 11,
        color: isSel ? accent : ok,
        outlineColor: Cesium.Color.WHITE,
        outlineWidth: 2,
      },
      label: {
        text: String(it.seq),
        font: '11px sans-serif',
        pixelOffset: new Cesium.Cartesian2(0, -16),
        fillColor: Cesium.Color.WHITE,
      },
    })
    layer.points.set(it.seq, entity)
  })
  return layer
}

/**
 * Move one rendered waypoint and its line vertex to a new position without
 * touching the mission store. Used for the live drag preview so a drag commits
 * once, on release (finding 14).
 */
export function previewWaypoint(
  layer: WaypointLayer,
  items: MissionItem[],
  seq: number,
  latDeg: number,
  lonDeg: number,
): void {
  const entity = layer.points.get(seq)
  if (!entity) return
  const item = items.find((it) => it.seq === seq)
  if (!item) return
  const height = item.z + 5
  entity.position = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(lonDeg, latDeg, height),
  )
  if (layer.line?.polyline) {
    layer.line.polyline.positions = new Cesium.ConstantProperty(
      items.map((it) =>
        it.seq === seq
          ? Cesium.Cartesian3.fromDegrees(lonDeg, latDeg, height)
          : Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z + 5),
      ),
    )
  }
}

/** A temporary point under the cursor while the add tool is active. */
export interface GhostPoint {
  entity: Cesium.Entity
}

/** Create the hidden hover marker for the add-waypoint tool. */
export function createGhostPoint(viewer: Cesium.Viewer): GhostPoint {
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const entity = viewer.entities.add({
    show: false,
    point: {
      pixelSize: 10,
      color: accent.withAlpha(0.5),
      outlineColor: Cesium.Color.WHITE,
      outlineWidth: 1,
    },
  })
  return { entity }
}

/** Position the hover marker, or hide it for `null` (cursor over the sky). */
export function setGhostPoint(ghost: GhostPoint, ground: LatLonLike | null): void {
  if (!ground) {
    ghost.entity.show = false
    return
  }
  ghost.entity.position = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(ground.lon, ground.lat, 0),
  )
  ghost.entity.show = true
}
