// @vitest-environment node

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const acceptanceScripts = [
  "android-catalog-acceptance.ts",
  "android-playback-acceptance.ts",
  "release-acceptance.ts",
] as const;

describe("Android device selector privacy", () => {
  it("keeps the private serial in ANDROID_SERIAL and out of every process argv", async () => {
    for (const name of acceptanceScripts) {
      const source = await readFile(
        fileURLToPath(new URL(name, import.meta.url)),
        "utf8",
      );
      expect(source).toContain("process.env.ANDROID_SERIAL");
      expect(source).not.toMatch(/["']--serial["']/u);
      expect(source).not.toMatch(
        /\[\s*["']-s["']\s*,\s*(?:this\.)?serial/gu,
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
