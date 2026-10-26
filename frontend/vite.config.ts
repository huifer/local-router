import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: "assets/app.js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: (info) => info.names?.[0]?.endsWith(".css") ? "assets/style.css" : "assets/[name][extname]",
      },
    },
  },
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:3426",
      "/health": "http://127.0.0.1:3425",
      "/v1": "http://127.0.0.1:3425",
      "/v1beta": "http://127.0.0.1:3425",
    },
  },
});
