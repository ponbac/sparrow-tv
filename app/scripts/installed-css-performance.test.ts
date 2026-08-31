// @vitest-environment node

import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import postcss, { type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const installedStylesRoot = fileURLToPath(new URL("../src/", import.meta.url));

describe("installed app repaint contract", () => {
  it("keeps persistent chrome static", async () => {
    const continuousChromeSelectors: string[] = [];

    for (const [path, stylesheet] of await installedStylesheets()) {
      stylesheet.walkRules((rule) => {
        if (!rule.selectors.some(containsSignalLamp)) return;

        rule.walkDecls((declaration) => {
          if (
            (declaration.prop === "animation" ||
              declaration.prop === "animation-iteration-count") &&
            /\binfinite\b/u.test(declaration.value)
          ) {
            continuousChromeSelectors.push(`${path}: ${rule.selector}`);
          }
        });
      });
    }

    expect(continuousChromeSelectors).toEqual([]);
  });

  it("has no fixed full-window blend layer", async () => {
    const fullWindowBlendSelectors: string[] = [];

    for (const [path, stylesheet] of await installedStylesheets()) {
      stylesheet.walkRules((rule) => {
        const declarations = declarationsByProperty(rule);
        const blendMode = declarations.get("mix-blend-mode");
        if (
          declarations.get("position") === "fixed" &&
          coversViewport(declarations) &&
          blendMode !== undefined &&
          blendMode !== "normal"
        ) {
          fullWindowBlendSelectors.push(`${path}: ${rule.selector}`);
        }
      });
    }

    expect(fullWindowBlendSelectors).toEqual([]);
  });
});

async function installedStylesheets(): Promise<
  readonly (readonly [string, postcss.Root])[]
> {
  const entries = await readdir(installedStylesRoot, { recursive: true });
  return Promise.all(
    entries
      .filter((path) => path.endsWith(".css"))
      .sort()
      .map(async (path) => [
        path,
        postcss.parse(await readFile(join(installedStylesRoot, path), "utf8")),
      ] as const),
  );
}

function containsSignalLamp(selector: string): boolean {
  return /(^|[\s>+~,(])\.signal-lamp(?=$|[\s>+~.:#,[)])/u.test(selector);
}

function declarationsByProperty(rule: Rule): ReadonlyMap<string, string> {
  const declarations = new Map<string, string>();
  rule.walkDecls((declaration) => {
    declarations.set(declaration.prop, declaration.value.trim());
  });
  return declarations;
}

function coversViewport(
  declarations: ReadonlyMap<string, string>,
): boolean {
  if (declarations.get("inset") === "0") return true;

  return ["top", "right", "bottom", "left"].every(
    (edge) => declarations.get(edge) === "0",
  );
}
