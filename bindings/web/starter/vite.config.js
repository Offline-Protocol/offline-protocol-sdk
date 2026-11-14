import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const starterRoot = path.dirname(fileURLToPath(import.meta.url));
const webBinding = path.resolve(starterRoot, "..");
const repoRoot = path.resolve(webBinding, "../..");

export default defineConfig({
  root: starterRoot,
  server: {
    port: 5173,
    strictPort: true,
    fs: {
      allow: [starterRoot, webBinding, repoRoot],
    },
  },
  resolve: {
    alias: {
      "@offline-protocol/web": path.join(webBinding, "src/index.js"),
    },
  },
  assetsInclude: ["**/*.wasm"],
});
