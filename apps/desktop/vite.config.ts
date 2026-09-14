import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: ["es2021", "chrome100", "safari13"],
    rolldownOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("/node_modules/three/")) return "three";
          if (id.includes("/node_modules/@xterm/")) return "terminal";
          if (id.includes("/node_modules/lucide-react/")) return "icons";
          if (id.includes("/node_modules/react/") || id.includes("/node_modules/react-dom/")) return "react";
        },
      },
    },
  },
});
