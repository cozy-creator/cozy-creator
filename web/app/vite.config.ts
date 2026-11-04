import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

// One bundle with relative paths: Calcifer serves `/` and `/assets/*`, and any static host can too.
export default defineConfig({
  base: "./",
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: "assets",
    assetsInlineLimit: 0,
    cssCodeSplit: false,
    modulePreload: false,
  },
})
