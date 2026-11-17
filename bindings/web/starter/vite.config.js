import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const starterRoot = path.dirname(fileURLToPath(import.meta.url));
const webBinding = path.resolve(starterRoot, "..");

export default defineConfig({
  root: starterRoot,
  server: {
    // The page, the worker and the module all live under bindings/web
    // (the starter included); the dev server serves nothing above it.
    fs: {
      allow: [webBinding],
    },
  },
  resolve: {
    alias: {
      "@offline-protocol/web": path.join(webBinding, "src/index.js"),
    },
  },
  assetsInclude: ["**/*.wasm"],
  worker: {
    format: "es",
  },
});
