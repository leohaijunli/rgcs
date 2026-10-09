// Cesium entity/layer construction: drone, trail, forward projection, home and
// the mission waypoint layer. Kept free of React so it can be unit-reasoned and
// reused; `MapView` only wires it up (issues.md #29).

import * as Cesium from 'cesium'
import { cssVar } from '../design-system/theme'
import i18n from '../i18n'
import { degFromMavInt } from '../stores/mission'
import type { GeoPoint } from '../generated-types/GeoPoint'
import type { MissionItem } from '../generated-types/MissionItem'
import type { LineKind } from '../generated-types/LineKind'
import { heightRuns, splitRuns, type KindBySeq } from '../mission/lineKinds'
import { selfIntersects } from '../mission/polygon'
import { HOME_LAT, HOME_LON, UAV_MODEL_URI } from './constants'

/** Below this (m) a waypoint is level with the lowest one; no stick is drawn. */
const HEIGHT_EPSILON_M = 0.25

/** Palette used for a run of waypoints; `cal` names the cloverleaf colour. */
interface KindPalette {
  accent: Cesium.Color
  ok: Cesium.Color
  warn: Cesium.Color
  cal: Cesium.Color
}

/**
 * Colour per pattern line kind: survey lines are the accent colour, tie lines
 * amber and the calibration cloverleaf green, so the three are distinguishable
 * at a glance (issues.md #35).
 */
function colorForKind(kind: LineKind | null, palette: KindPalette): Cesium.Color {
  switch (kind) {
    case 'survey':
      return palette.accent
    case 'tie':
      return palette.warn
    case 'calibration':
      return palette.cal
    default:
      return palette.ok
  }
}

/** Initial position (degrees) for the placeholder fix and the HOME marker. */
interface LatLonLike {
  lat: number
  lon: number
}

/**
 * Vertical line from the vehicle down to the ground plus its AGL readout, so
 * the height above ground is visible in the scene and not only in the HUD
 * (issues.md #40).
 */
export interface DropLayer {
  line: Cesium.Entity
  label: Cesium.Entity
}

/** The fixed drone/trail/predict/home layer, created once per viewer. */
export interface DroneLayer {
  drone: Cesium.Entity
  trail: Cesium.Entity
  predict: Cesium.Entity
  home: Cesium.Entity
  drop: DropLayer
  /** Created once and updated in place; swapping properties rebuilds the model. */
  dronePosition: Cesium.ConstantPositionProperty
  droneOrientation: Cesium.ConstantProperty
}

/**
 * Add the static layers. The drone is hidden until the first fix: the map has
 * no terrain provider, so an untagged placeholder would sit in the air above
 * the ellipsoid ground.
 *
 * `homeAmslM` is the ground-plane altitude everything is drawn against: the
 * plan is rendered in AMSL (`MapView.toDisplayItems`), so HOME and the drop
 * line stand at HOME's AMSL and the vehicle is placed at `HOME + relative_alt`.
 */
export function createDroneLayer(
  viewer: Cesium.Viewer,
  trailPos: () => Cesium.Cartesian3[],
  predictPos: () => Cesium.Cartesian3[],
  initial: LatLonLike = { lat: HOME_LAT, lon: HOME_LON },
  homeAmslM = 0,
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
  // Ground drop line: from the vehicle straight down to the ground plane the
  // plan is drawn on (HOME's AMSL altitude while there is no DEM). Hidden until
  // the first fix and when the vehicle is on the ground.
  const dropLine = viewer.entities.add({
    show: false,
    polyline: {
      positions: Cesium.Cartesian3.fromDegreesArrayHeights([
        initial.lon,
        initial.lat,
        homeAmslM,
        initial.lon,
        initial.lat,
        homeAmslM,
      ]),
      width: 3,
      material: new Cesium.PolylineDashMaterialProperty({
        color: warn.withAlpha(0.9),
        dashLength: 12,
      }),
    },
  })
  const dropLabel = viewer.entities.add({
    show: false,
    position: Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, homeAmslM),
    label: {
      text: '',
      font: '11px sans-serif',
      fillColor: Cesium.Color.WHITE,
      pixelOffset: new Cesium.Cartesian2(0, 10),
      disableDepthTestDistance: Number.POSITIVE_INFINITY,
    },
  })
  const drop: DropLayer = { line: dropLine, label: dropLabel }

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
    position: Cesium.Cartesian3.fromDegrees(initial.lon, initial.lat, homeAmslM),
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
  return { drone, trail, predict, home, drop, dronePosition, droneOrientation }
}

/** Below this the drop line is noise: the vehicle is on the ground. */
const DROP_MIN_AGL_M = 1.5

/**
 * Point the AGL drop line straight down from the vehicle to the ground plane
 * and label it with the height above ground.
 *
 * Both heights are in the same datum the plan is drawn in (AMSL): `topM` is
 * the vehicle's altitude, `bottomM` the ground-plane altitude (HOME's AMSL
 * until a DEM exists), so the line ends exactly where the height sticks stand.
 */
export function updateDropLine(
  drop: DropLayer,
  lonDeg: number,
  latDeg: number,
  topM: number,
  bottomM: number,
): void {
  const aglM = topM - bottomM
  const visible = Number.isFinite(aglM) && aglM >= DROP_MIN_AGL_M
  drop.line.show = visible
  drop.label.show = visible
  if (!visible) return
  const polyline = drop.line.polyline
  if (polyline) {
    polyline.positions = new Cesium.ConstantProperty(
      Cesium.Cartesian3.fromDegreesArrayHeights([lonDeg, latDeg, topM, lonDeg, latDeg, bottomM]),
    )
  }
  drop.label.position = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(lonDeg, latDeg, bottomM),
  )
  if (drop.label.label) {
    drop.label.label.text = new Cesium.ConstantProperty(
      i18n.t('map.aglLabel', { value: aglM.toFixed(1) }),
    )
  }
}

/** Rendered mission waypoints: the polylines, one point per item, and the
 * height sticks. */
export interface WaypointLayer {
  /** One polyline per same-kind run, with the seqs it connects (for drag preview). */
  runs: Array<{ entity: Cesium.Entity; seqs: number[] }>
  points: Map<number, Cesium.Entity>
  extra: Cesium.Entity[]
}

/** How to colour the plan: pattern line kinds and the height sticks. */
export interface WaypointRenderOptions {
  /** Waypoint `seq` to survey/tie/calibration line kind (see lineKinds.ts). */
  kinds?: KindBySeq
  /** Draw a height stick and an altitude label per waypoint. */
  heights?: boolean
  /** Ground AMSL altitude to stand the height sticks on (HOME while no DEM). */
  groundM?: number | null
}

/** Remove a previously rendered waypoint layer from the viewer. */
export function disposeWaypointLayer(viewer: Cesium.Viewer, layer: WaypointLayer | null): void {
  if (!layer) return
  layer.points.forEach((e) => viewer.entities.remove(e))
  layer.points.clear()
  layer.extra.forEach((e) => viewer.entities.remove(e))
  layer.extra = []
  for (const run of layer.runs) viewer.entities.remove(run.entity)
  layer.runs = []
}

/** Draw the mission items as a glowing polyline plus numbered, selectable points. */
export function renderWaypoints(
  viewer: Cesium.Viewer,
  items: MissionItem[],
  selectedSeq: number | null,
  opts: WaypointRenderOptions = {},
): WaypointLayer {
  const layer: WaypointLayer = { runs: [], points: new Map(), extra: [] }
  // Command items (`MAV_FRAME_MISSION`) carry no position: their x/y are
  // command arguments, so drawing them would put a vertex at the equator.
  const located = items.filter((it) => it.frame !== 'mission')
  if (located.length === 0) return layer

  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const ok = Cesium.Color.fromCssColorString(cssVar('--mg-ok'))
  const warn = Cesium.Color.fromCssColorString(cssVar('--mg-warn'))
  // Calibration reuses the purple token (it belongs to the mag palette, which
  // no other map layer uses yet) so all three pattern kinds are distinguishable.
  const cal = Cesium.Color.fromCssColorString(cssVar('--mg-mag'))
  const palette: KindPalette = { accent, ok, warn, cal }
  const kinds = opts.kinds ?? new Map<number, LineKind>()

  // Height sticks first, so the path and the points stay on top of them. The
  // stick runs from the ground (HOME altitude until there is a DEM) up to the
  // waypoint, so the operator reads the clearance directly (issues.md #36).
  if (opts.heights) {
    const muted = Cesium.Color.fromCssColorString(cssVar('--mg-muted'))
    for (const run of heightRuns(located, opts.groundM ?? null)) {
      if (Math.abs(run.top - run.base) < HEIGHT_EPSILON_M) continue
      const item = located.find((it) => it.seq === run.seq)
      if (!item) continue
      layer.extra.push(
        viewer.entities.add({
          polyline: {
            positions: [
              Cesium.Cartesian3.fromDegrees(degFromMavInt(item.y), degFromMavInt(item.x), run.base),
              Cesium.Cartesian3.fromDegrees(degFromMavInt(item.y), degFromMavInt(item.x), run.top),
            ],
            width: 1,
            material: muted.withAlpha(0.7),
          },
        }),
      )
    }
  }

  // One polyline per run of same-kind waypoints, so a tie line is visibly a
  // different colour from the survey lines it links (issues.md #35).
  for (const run of splitRuns(located, kinds)) {
    if (run.items.length < 2) continue
    const colour = colorForKind(run.kind, palette)
    const entity = viewer.entities.add({
      polyline: {
        positions: run.items.map((it) =>
          Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z),
        ),
        width: run.kind === 'tie' ? 3 : 2,
        material: new Cesium.PolylineGlowMaterialProperty({
          glowPower: 0.18,
          color: colour.withAlpha(0.85),
        }),
      },
    })
    layer.runs.push({ entity, seqs: run.items.map((it) => it.seq) })
  }

  located.forEach((it) => {
    const isSel = it.seq === selectedSeq
    const kind = kinds.get(it.seq) ?? null
    const position = Cesium.Cartesian3.fromDegrees(
      degFromMavInt(it.y),
      degFromMavInt(it.x),
      it.z,
    )
    // A selected waypoint gets a white halo ring on top of a larger marker:
    // recolouring alone is invisible when the whole plan already shares the
    // accent colour, which is what the operator reported (issues.md #37).
    if (isSel) {
      layer.extra.push(
        viewer.entities.add({
          position,
          point: {
            pixelSize: 26,
            color: Cesium.Color.TRANSPARENT,
            outlineColor: Cesium.Color.WHITE,
            outlineWidth: 3,
          },
        }),
      )
    }
    const entity = viewer.entities.add({
      id: `wp-${it.seq}`,
      position,
      point: {
        pixelSize: isSel ? 16 : 11,
        color: isSel ? accent : colorForKind(kind, palette),
        outlineColor: Cesium.Color.WHITE,
        outlineWidth: isSel ? 3 : 2,
      },
      label: {
        // Altitude in the same units the panel edits (AMSL), so a waypoint's
        // height is readable without opening the editor (issues.md #36).
        text: opts.heights ? `${it.seq} · ${Math.round(it.z)} m` : String(it.seq),
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
 * Move one rendered waypoint and the polyline vertices that touch it without
 * touching the mission store. Used for the live drag preview so a drag commits
 * once, on release (finding 14). The plan may be split into several coloured
 * runs, so every run that contains this seq is redrawn (only one can).
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
  const height = item.z
  entity.position = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(lonDeg, latDeg, height),
  )
  for (const run of layer.runs) {
    if (!run.seqs.includes(seq)) continue
    const polyline = run.entity.polyline
    if (!polyline) continue
    polyline.positions = new Cesium.ConstantProperty(
      run.seqs.map((s) => {
        if (s === seq) return Cesium.Cartesian3.fromDegrees(lonDeg, latDeg, height)
        const it = items.find((i) => i.seq === s)
        if (!it) return Cesium.Cartesian3.fromDegrees(lonDeg, latDeg, height)
        return Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z)
      }),
    )
  }
}

/** A temporary point under the cursor while the add tool is active. */
export interface GhostPoint {
  /** The point itself, previewing the waypoint at its inherited altitude. */
  entity: Cesium.Entity
  /** Ground stick from the clicked ground point up to the previewed altitude. */
  stick: Cesium.Entity
}

/** Create the hidden hover marker plus its ground stick for the add tool. */
export function createGhostPoint(viewer: Cesium.Viewer): GhostPoint {
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const muted = Cesium.Color.fromCssColorString(cssVar('--mg-muted'))
  const entity = viewer.entities.add({
    show: false,
    point: {
      pixelSize: 10,
      color: accent.withAlpha(0.5),
      outlineColor: Cesium.Color.WHITE,
      outlineWidth: 1,
    },
  })
  const stick = viewer.entities.add({
    show: false,
    polyline: {
      positions: [],
      width: 1,
      material: muted.withAlpha(0.5),
    },
  })
  return { entity, stick }
}

/**
 * Position the hover marker at the altitude a new waypoint would get (`topM`),
 * with a ground stick down to the clicked ground plane (`bottomM`), so the
 * preview and the placed waypoint coincide even from a tilted 3D camera.
 * `null` hides it (cursor over the sky).
 */
export function setGhostPoint(
  ghost: GhostPoint,
  ground: LatLonLike | null,
  topM: number,
  bottomM: number,
): void {
  if (!ground) {
    ghost.entity.show = false
    ghost.stick.show = false
    return
  }
  ghost.entity.position = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(ground.lon, ground.lat, topM),
  )
  ghost.entity.show = true
  const stick = ghost.stick.polyline
  if (stick) {
    stick.positions = new Cesium.ConstantProperty(
      Cesium.Cartesian3.fromDegreesArrayHeights([
        ground.lon,
        ground.lat,
        topM,
        ground.lon,
        ground.lat,
        bottomM,
      ]),
    )
  }
  ghost.stick.show = true
}

/** Draft survey-boundary polygon: vertex markers (id `pg-<i>`) plus the edge. */
export interface PolygonLayer {
  vertices: Cesium.Entity[]
  boundary: Cesium.Entity
}

/** Create the (hidden) boundary polyline and an empty vertex set. */
export function createPolygonLayer(viewer: Cesium.Viewer): PolygonLayer {
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const boundary = viewer.entities.add({
    show: false,
    polyline: {
      positions: [],
      width: 3,
      material: new Cesium.PolylineGlowMaterialProperty({
        glowPower: 0.2,
        color: accent.withAlpha(0.9),
      }),
    },
  })
  return { vertices: [], boundary }
}

/**
 * Rebuild the draft polygon: vertex markers, the connecting edge, and the
 * error colour when it self-intersects. Pass an empty list to hide it.
 */
export function updatePolygonLayer(
  viewer: Cesium.Viewer,
  layer: PolygonLayer,
  vertices: readonly GeoPoint[],
  closed: boolean,
): void {
  for (const e of layer.vertices) viewer.entities.remove(e)
  layer.vertices = []
  if (vertices.length === 0) {
    layer.boundary.show = false
    return
  }
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const error = Cesium.Color.fromCssColorString(cssVar('--mg-error'))
  // Vertex markers render from the first click on; the connecting edge needs
  // at least two vertices, so a single-vertex draft stays visible.
  vertices.forEach((p, i) => {
    layer.vertices.push(
      viewer.entities.add({
        id: `pg-${i}`,
        position: Cesium.Cartesian3.fromDegrees(p.longitude_deg, p.latitude_deg, 0),
        point: {
          pixelSize: i === 0 ? 12 : 9,
          color: i === 0 ? Cesium.Color.WHITE : accent,
          outlineColor: Cesium.Color.WHITE,
          outlineWidth: 2,
        },
      }),
    )
  })
  if (vertices.length < 2) {
    layer.boundary.show = false
    return
  }
  const points = vertices.map((p) =>
    Cesium.Cartesian3.fromDegrees(p.longitude_deg, p.latitude_deg, 0),
  )
  if (closed && vertices.length >= 3) points.push(Cesium.Cartesian3.clone(points[0]))
  layer.boundary.show = true
  const bad = closed && selfIntersects(vertices)
  if (layer.boundary.polyline) {
    layer.boundary.polyline.positions = new Cesium.ConstantProperty(points)
    layer.boundary.polyline.material = new Cesium.PolylineGlowMaterialProperty({
      glowPower: 0.2,
      color: (bad ? error : accent).withAlpha(0.9),
    })
  }
}

/** Remove the draft polygon from the viewer. */
export function disposePolygonLayer(viewer: Cesium.Viewer, layer: PolygonLayer | null): void {
  if (!layer) return
  for (const e of layer.vertices) viewer.entities.remove(e)
  layer.vertices = []
  viewer.entities.remove(layer.boundary)
}

/** Draggable centre marker for the last generated pattern (A2). */
export interface CenterHandle {
  entity: Cesium.Entity
}

/** Create the hidden pattern-centre handle (id `pattern-center`). */
export function createCenterHandle(viewer: Cesium.Viewer): CenterHandle {
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const entity = viewer.entities.add({
    show: false,
    id: 'pattern-center',
    point: {
      pixelSize: 12,
      color: Cesium.Color.TRANSPARENT,
      outlineColor: accent,
      outlineWidth: 3,
    },
    label: {
      text: i18n.t('map.patternCenter'),
      font: '11px sans-serif',
      pixelOffset: new Cesium.Cartesian2(0, -18),
      fillColor: Cesium.Color.WHITE,
      disableDepthTestDistance: Number.POSITIVE_INFINITY,
    },
  })
  return { entity }
}

/** Position or hide the centre handle; it sits on the pattern's ground plane. */
export function updateCenterHandle(
  handle: CenterHandle,
  center: GeoPoint | null,
  groundM: number,
): void {
  if (!center) {
    handle.entity.show = false
    return
  }
  handle.entity.position = new Cesium.ConstantPositionProperty(
    Cesium.Cartesian3.fromDegrees(center.longitude_deg, center.latitude_deg, groundM),
  )
  handle.entity.show = true
}
