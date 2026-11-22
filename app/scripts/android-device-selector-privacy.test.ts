// @vitest-environment node

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const deviceScripts = [
  "android-catalog-acceptance.ts",
  "android-playback-acceptance.ts",
  "release-acceptance.ts",
  "../../scripts/debug/shield-benchmark.mjs",
  "../../scripts/debug/shield-startup.mjs",
  "../../scripts/debug/shield-startup.py",
] as const;

describe("Android device selector privacy", () => {
  it("keeps the private serial in ANDROID_SERIAL and out of every process argv", async () => {
    for (const name of deviceScripts) {
      const source = await readFile(
        fileURLToPath(new URL(name, import.meta.url)),
        "utf8",
      );
      expect(source).toMatch(/process\.env\.ANDROID_SERIAL|os\.environ\.get\(["']ANDROID_SERIAL["']\)/u);
      expect(source).not.toMatch(/["']--serial["']/u);
      expect(source).not.toMatch(
        /["']-s["']\s*,\s*(?:this\.)?serial\b/u,
      );
    }

    const justfile = await readFile(
      fileURLToPath(new URL("../../justfile", import.meta.url)),
      "utf8",
    );
    for (const recipe of [
      "release-acceptance-prove-continuity",
      "android-catalog-accept",
      "android-playback-accept",
    ]) {
      const body = recipeBody(justfile, recipe);
      expect(body).toContain("ANDROID_SERIAL");
      expect(body).not.toContain("--serial");
    }
  });
});

function recipeBody(justfile: string, recipe: string): string {
  const match = new RegExp(
    `^${recipe}:\\n(?<body>(?:[ \\t].*(?:\\n|$))+)`,
    "mu",
  ).exec(justfile);
  expect(match, `${recipe} must remain in the justfile`).not.toBeNull();
  return match?.groups?.body ?? "";
}
