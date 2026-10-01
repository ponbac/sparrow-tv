// @vitest-environment node

import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import postcss, { type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const installedStylesRoot = fileURLToPath(new URL("../src/", import.meta.url));
const THEATER_STYLESHEET = "features/stage/theater.css";
const THEATER_SCOPE = '.shell[data-layout="theater"]';

describe("installed app repaint contract", () => {
  it("scales shell chrome with large-desktop typography", async () => {
    const stylesheets = new Map(await installedStylesheets());
    const indexStyles = requireStylesheet(stylesheets, "index.css");
    const shellStyles = requireStylesheet(
      stylesheets,
      "features/stage/shell.css",
    );
    const stageStyles = requireStylesheet(
      stylesheets,
      "features/stage/stage.css",
    );
    const guideStyles = requireStylesheet(
      stylesheets,
      "features/guide/programme-guide.css",
    );
    const theaterStyles = requireStylesheet(stylesheets, THEATER_STYLESHEET);

    expect(declaration(indexStyles, ":root", "--guide-gutter")).toBe(
      "min(20rem, 38%)",
    );
    // Android dialogs anchor below the stacked masthead at this height.
    expect(declaration(indexStyles, ":root", "--bar-h")).toBe("2.375rem");
    expect(declaration(indexStyles, "html", "font-size")).toBe(
      "clamp(100%, 0.8vw, 150%)",
    );
    expect(declaration(shellStyles, ".shell", "font-size")).toBe("0.8125rem");
    expect(declaration(shellStyles, ".shell", "grid-template-rows")).toBe(
      "2.375rem auto minmax(0, 1fr)",
    );
    expect(
      declaration(guideStyles, ".programme-guide", "grid-template-rows"),
    ).toBe("2.75rem minmax(0, 1fr)");
    expect(declaration(guideStyles, ".programme-guide__row", "height")).toBe(
      "2.75rem",
    );
    expect(declaration(stageStyles, ".stage", "grid-template-rows")).toBe(
      "auto auto",
    );
    // Theater has its own masthead and its own, taller guide rows; the
    // stacked values above are untouched by it.
    expect(
      declaration(theaterStyles, `:root:has(${THEATER_SCOPE})`, "--bar-h"),
    ).toBe("3.5rem");
    expect(
      declaration(
        theaterStyles,
        `${THEATER_SCOPE} .programme-guide__row`,
        "height",
      ),
    ).toBe("3.7rem");
  });

  it("keeps persistent shell chrome static", async () => {
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

  it("never blurs what lies behind a panel", async () => {
    const blurred: string[] = [];

    for (const [path, stylesheet] of await installedStylesheets()) {
      stylesheet.walkRules((rule) => {
        rule.walkDecls(/^(-webkit-)?backdrop-filter$/u, () => {
          blurred.push(`${path}: ${rule.selector}`);
        });
      });
    }

    expect(blurred).toEqual([]);
  });

  it("scopes every Theater rule to the Theater layout", async () => {
    const theaterStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      THEATER_STYLESHEET,
    );
    const unscoped: string[] = [];
    let rules = 0;

    theaterStyles.walkRules((rule) => {
      rules += 1;
      // In-tree elements hang off the shell; portalled ones off the root
      // that holds it.
      unscoped.push(
        ...rule.selectors.filter(
          (selector) => !selector.includes(THEATER_SCOPE),
        ),
      );
    });

    expect(rules).toBeGreaterThan(0);
    expect(unscoped).toEqual([]);
  });

  it("lets Theater change nothing over time but opacity", async () => {
    const theaterStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      THEATER_STYLESHEET,
    );
    const moving: string[] = [];

    theaterStyles.walkAtRules("keyframes", (keyframes) => {
      moving.push(`@keyframes ${keyframes.params}`);
    });
    theaterStyles.walkRules((rule) => {
      rule.walkDecls((declaration) => {
        if (
          declaration.prop.startsWith("animation") ||
          ((declaration.prop === "transition" ||
            declaration.prop === "transition-property") &&
            !transitionsOnlyOpacity(declaration.value))
        ) {
          moving.push(`${rule.selector} { ${declaration} }`);
        }
      });
    });

    expect(moving).toEqual([]);
  });

  it("never transitions or animates the picture's box", async () => {
    const moving: string[] = [];

    for (const [path, stylesheet] of await installedStylesheets()) {
      stylesheet.walkRules((rule) => {
        if (!rule.selectors.some(containsPicture)) return;
        rule.walkDecls(/^(transition|animation)/u, (declaration) => {
          moving.push(`${path}: ${rule.selector} { ${declaration} }`);
        });
      });
    }

    expect(moving).toEqual([]);
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

function requireStylesheet(
  stylesheets: ReadonlyMap<string, postcss.Root>,
  path: string,
): postcss.Root {
  const stylesheet = stylesheets.get(path);
  if (stylesheet === undefined) {
    throw new Error(`Missing stylesheet: ${path}`);
  }
  return stylesheet;
}

function containsPersistentChrome(selector: string): boolean {
  return /(^|[\s>+~,(])\.shell__(?:masthead|wordmark|clock|status)(?=$|[\s>+~.:#,[)])/u.test(
    selector,
  );
}

/** The picture's containers and the video element itself. */
function containsPicture(selector: string): boolean {
  return /(^|[\s>+~,(])(?:\.stage__monitor|\.hosted-player__screen|video)(?=$|[\s>+~.:#,[)])/u.test(
    selector,
  );
}

/**
 * Whether a `transition` or `transition-property` value names opacity and
 * nothing else. A value that names no property transitions everything.
 */
function transitionsOnlyOpacity(value: string): boolean {
  return value
    .split(",")
    .every((transition) => /^opacity(\s|$)/u.test(transition.trim()));
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
    // Desktop defaults exclude conditional phone and accessibility overrides.
    if (rule.parent?.type !== "root") return;
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
