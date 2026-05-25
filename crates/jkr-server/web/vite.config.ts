import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";

// Build output is a single HTML file with all JS + CSS inlined, written
// directly into ../static/index.html. jkr-server then `include_str!`s that
// file at compile time — no asset pipeline at runtime.
export default defineConfig({
  plugins: [react(), viteSingleFile()],
  build: {
    outDir: "../static",
    emptyOutDir: false,
    target: "es2020",
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
        manualChunks: undefined,
      },
    },
  },
  server: {
    port: 5173,
    proxy: {
      // During `npm run dev`, proxy API calls to the local jkr-server.
      "/api": "http://127.0.0.1:4000",
    },
  },
});
