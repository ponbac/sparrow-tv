import { mergeConfig } from "vite";
import base from "./vite.config";

// The guide bench: the TV shell over a deterministic catalog, built like a release.
export default mergeConfig(base, {
  base: "/",
  publicDir: process.env.BENCH_PUBLIC ?? false,
  build: {
    outDir: "dist-bench",
    minify: process.env.BENCH_MINIFY !== "0",
    rollupOptions: { input: "bench.html" },
  },
});
