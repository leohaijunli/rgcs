import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { viteStaticCopy } from 'vite-plugin-static-copy'

// Cesium workers/assets are served as static files (no online assets in the field).
const cesium = 'node_modules/cesium/Build/Cesium'

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    viteStaticCopy({
      targets: [
        { src: `${cesium}/Workers`, dest: 'cesium' },
        { src: `${cesium}/ThirdParty`, dest: 'cesium' },
        { src: `${cesium}/Assets`, dest: 'cesium' },
        { src: `${cesium}/Widgets`, dest: 'cesium' },
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