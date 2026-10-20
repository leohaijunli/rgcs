# UAV model assets

Source 3D asset used for the map's UAV marker.

## Attribution

- **Title:** animated drone with camera (FREE)
- **Author:** ulunkwulunk (https://sketchfab.com/ulunkwulunk)
- **Source:** https://sketchfab.com/3d-models/animated-drone-with-camera-free-a8e2c50f69264e75bb6277779fb5028b
- **License:** CC-BY-4.0 (http://creativecommons.org/licenses/by/4.0/)

CC-BY-4.0 requires attribution. Keep this file and the license alongside the
asset, and credit the author wherever the model is redistributed.

## Files

| Path | Committed | Purpose |
| --- | --- | --- |
| `scene.gltf`, `scene.bin`, `textures/` | yes | Original Sketchfab export (source). |
| `LICENSE.txt` | yes | Asset license as shipped by the author. |

## Runtime copy

The app does not load the original export. `frontend/scripts/build-uav-model.mjs`
converts it into a static (non-skinned) model served from
`frontend/public/model/`:

- `scene-static.gltf`, `scene-static.bin` — generated
- `textures/*.png` — copied from this folder

Cesium's glTF loader builds draw commands for the source's skinned primitives
but does not rasterize them, so the drone is invisible. The build strips
skins/animations plus the attributes only skinning needs (`JOINTS_0`,
`WEIGHTS_0`) and repacks the attributes the materials actually use into a tight
buffer (8.4 MB → 2.8 MB).

## Regenerating

```bash
cd frontend
node scripts/build-uav-model.mjs
```
