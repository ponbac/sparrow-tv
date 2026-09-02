// @vitest-environment node

import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import postcss, { type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const installedStylesRoot = fileURLToPath(new URL("../src/", import.meta.url));

describe("installed app repaint contract", () => {
  it("scales Split Stage chrome with large-desktop typography", async () => {
    const stylesheets = new Map(await installedStylesheets());
    const indexStyles = stylesheets.get("index.css");
    const splitStageStyles = stylesheets.get("features/guide/split-stage.css");
    const guideStyles = stylesheets.get("features/guide/programme-guide.css");
    const cinemaStyles = stylesheets.get("features/guide/cinema-stage.css");

    expect(indexStyles).toBeDefined();
    expect(splitStageStyles).toBeDefined();
    expect(guideStyles).toBeDefined();
    expect(cinemaStyles).toBeDefined();

    expect(declaration(indexStyles!, "html", "font-size")).toBe(
      "clamp(100%, 0.75vw, 150%)",
    );
    expect(declaration(splitStageStyles!, ".split-stage", "font-size")).toBe(
      "0.8125rem",
    );
    expect(
      declaration(splitStageStyles!, ".split-stage", "grid-template-rows"),
    ).toBe("2.375rem auto minmax(0, 1fr)");
    expect(
      declaration(guideStyles!, ".programme-guide", "grid-template-rows"),
    ).toBe("2.75rem minmax(0, 1fr)");
    expect(declaration(guideStyles!, ".programme-guide__row", "height")).toBe(
      "2.75rem",
    );
    expect(
      declaration(cinemaStyles!, ".cinema-stage", "grid-template-rows"),
    ).toBe("minmax(11.25rem, 1fr) auto 3.625rem");
  });

  it("keeps persistent Split Stage chrome static", async () => {
    const continuousChromeSelectors: string[] = [];
    let persistentChromeRules = 0;

    for (const [path, stylesheet] of await installedStylesheets()) {
      stylesheet.walkRules((rule) => {
        if (!rule.selectors.some(containsPersistentChrome)) return;
        persistentChromeRules += 1;

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

    expect(persistentChromeRules).toBeGreaterThan(0);
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

function containsPersistentChrome(selector: string): boolean {
  return /(^|[\s>+~,(])\.split-stage__(?:masthead|identity|freshness|status)(?=$|[\s>+~.:#,[)])/u.test(
    selector,
  );
}

function declarationsByProperty(rule: Rule): ReadonlyMap<string, string> {
  const declarations = new Map<string, string>();
  rule.walkDecls((declaration) => {
    declarations.set(declaration.prop, declaration.value.trim());
  });
  return declarations;
}

function declaration(
  stylesheet: postcss.Root,
  selector: string,
  property: string,
): string | undefined {
  let value: string | undefined;
  stylesheet.walkRules(selector, (rule) => {
    value = declarationsByProperty(rule).get(property) ?? value;
  });
  return value;
}

function coversViewport(
  declarations: ReadonlyMap<string, string>,
): boolean {
  if (declarations.get("inset") === "0") return true;

  return ["top", "right", "bottom", "left"].every(
    (edge) => declarations.get(edge) === "0",
  );
}
