import { defineConfig } from "vite";

// The dev server port is not a preference: src-tauri/tauri.conf.json waits on
// `build.devUrl` (http://localhost:1420), and Vite's own default is 5173. With
// no config here the two never meet and the app window never opens, which is
// what this file was missing (#18).
export default defineConfig({
  // Tauri prints its own progress; clearing the screen eats it.
  clearScreen: false,

  server: {
    port: 1420,
    // Fail loudly rather than silently sliding to 1421 when the port is taken.
    // A moved port is the same failure this file exists to prevent.
    strictPort: true,
    watch: {
      // Rust rebuilds are driven by cargo, not by the frontend watcher.
      ignored: ["**/src-tauri/**", "**/crates/**", "**/sidecar/**"],
    },
  },

  envPrefix: ["VITE_", "TAURI_"],
  build: {
    rollupOptions: { input: ["index.html", "html-preview.html"] },
  },
});
