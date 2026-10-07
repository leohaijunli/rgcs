import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { viteStaticCopy } from 'vite-plugin-static-copy'

// Cesium workers/assets are served as static files (no online assets in the field).
const cesium = 'node_modules/cesium/Build/Cesium'

// Only the Cesium asset subtrees this app actually reaches are copied. The
// default imagery (NaturalEarthII), the maki pin icons (PinBuilder is never
// used) and the ocean/water normal maps are dropped to trim the packaged app
// (issues.md #30). Add a subtree back here if a new layer starts requesting it.
const cesiumAssets = [
  { src: `${cesium}/Assets/approximateTerrainHeights.json`, dest: 'cesium/Assets' },
  // High-precision ICRF data; the default scene requests it for the sun/moon.
  { src: `${cesium}/Assets/IAU2006_XYS`, dest: 'cesium/Assets' },
  { src: `${cesium}/Assets/Images`, dest: 'cesium/Assets' },
  { src: `${cesium}/Assets/Textures/SkyBox`, dest: 'cesium/Assets/Textures' },
  { src: `${cesium}/Assets/Textures/LensFlare`, dest: 'cesium/Assets/Textures' },
  { src: `${cesium}/Assets/Textures/moonSmall.jpg`, dest: 'cesium/Assets/Textures' },
]

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    viteStaticCopy({
      targets: [
        { src: `${cesium}/Workers`, dest: 'cesium' },
        { src: `${cesium}/ThirdParty`, dest: 'cesium' },
        { src: `${cesium}/Widgets`, dest: 'cesium' },
        ...cesiumAssets,
      ],
    }),
  ],
  // Tauri expects a fixed dev port and no clearing of the console.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  build: {
    target: 'es2021',
    chunkSizeWarningLimit: 2048,
  },
})
