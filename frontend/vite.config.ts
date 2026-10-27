import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";
import { fileURLToPath } from "node:url";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
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
    // The gateway and the console share one port now, so everything the
    // console talks to goes to the same place.
    proxy: {
      "/api": "http://127.0.0.1:3435",
      "/health": "http://127.0.0.1:3435",
      "/v1": "http://127.0.0.1:3435",
      "/v1beta": "http://127.0.0.1:3435",
    },
  },
});
