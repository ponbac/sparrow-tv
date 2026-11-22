// @vitest-environment node

import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import postcss, { type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const installedStylesRoot = fileURLToPath(new URL("../src/", import.meta.url));
const THEATER_STYLESHEET = "features/stage/theater.css";
const THEATER_SCOPE = '.shell[data-layout="theater"]';
const POCKET_STYLESHEET = "features/stage/pocket.css";
const POCKET_SCOPE = '.shell[data-layout="pocket"]';
const SPLIT_POCKET = `${POCKET_SCOPE}:where([data-pocket="split"])`;
const STACKED_POCKET = `${POCKET_SCOPE}:where([data-pocket="stacked"])`;

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
    // Android dialogs anchor below the pocket masthead at this height.
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
    // pocket values above are untouched by it.
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

  it("tones Channel Icon plates without a filter or a blend", async () => {
    // A guide shows dozens of plates at once. A filter or a blend mode on
    // one makes it its own layer, redrawn with the list; the TV tones light
    // plates with a plain veil instead.
    const filtered: string[] = [];

    for (const [path, stylesheet] of await installedStylesheets()) {
      stylesheet.walkRules((rule) => {
        if (!rule.selectors.some((selector) => selector.includes(".channel-icon"))) return;
        rule.walkDecls(/^(-webkit-)?filter$|^mix-blend-mode$/u, (declaration) => {
          filtered.push(`${path}: ${rule.selector} { ${declaration} }`);
        });
      });
    }

    expect(filtered).toEqual([]);
  });

  it.each([
    [THEATER_STYLESHEET, THEATER_SCOPE],
    [POCKET_STYLESHEET, POCKET_SCOPE],
    ["features/stage/television.css", "html[data-television]"],
  ])("scopes every rule in %s to its own layout", async (path, scope) => {
    const layoutStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      path,
    );
    const unscoped: string[] = [];
    let rules = 0;

    layoutStyles.walkRules((rule) => {
      rules += 1;
      // In-tree elements hang off the shell; portalled ones off the root
      // that holds it.
      unscoped.push(
        ...rule.selectors.filter((selector) => !selector.includes(scope)),
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

  it("lets pocket change nothing over time", async () => {
    // Native Android video follows the picture's box late, and never between
    // two sizes: every change of the layout is a jump.
    const pocketStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      POCKET_STYLESHEET,
    );
    const moving: string[] = [];

    pocketStyles.walkAtRules("keyframes", (keyframes) => {
      moving.push(`@keyframes ${keyframes.params}`);
    });
    pocketStyles.walkRules((rule) => {
      rule.walkDecls(/^(transition|animation)/u, (declaration) => {
        moving.push(`${rule.selector} { ${declaration} }`);
      });
    });

    expect(moving).toEqual([]);
  });

  it("keeps what pocket raises inside the stage under the sheets", async () => {
    // A sheet's backdrop is a layer of the document with no z-index of its
    // own. Without a stacking context on the stage, the band's raised button
    // would lie over it and stay live under an open sheet.
    const pocketStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      POCKET_STYLESHEET,
    );

    expect(declaration(pocketStyles, `${POCKET_SCOPE} .stage`, "isolation")).toBe(
      "isolate",
    );
  });

  it("gives a short landscape window's control row one line of a fixed height", async () => {
    // The picture takes what the info block leaves there, and native Android
    // video does not follow its box while paused or failed: nothing the row
    // holds may change the block's height.
    const pocketStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      POCKET_STYLESHEET,
    );
    const controls = `${SPLIT_POCKET} .hosted-player__controls[data-variant="compact"]`;

    expect(
      declaration(pocketStyles, `${SPLIT_POCKET} .now-playing__controls`, "height"),
    ).toBe("calc(2.75rem + 1px)");
    expect(declaration(pocketStyles, controls, "flex-wrap")).toBe("nowrap");
    expect(
      declaration(pocketStyles, `${controls} > .hosted-player__status`, "width"),
    ).toBe("auto");
  });

  it("sizes pocket's picture, and what is placed by it, from the size it is held at", async () => {
    // While native Android video cannot follow its box, the shell writes the
    // box's size to the root. A rule that sized the box, the guide beside it
    // or a sheet under it without reading that size would move the page
    // under a picture that stays where it is.
    const pocketStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      POCKET_STYLESHEET,
    );
    const playing = `${POCKET_SCOPE.slice(0, -1)}][data-playing="true"]`;
    const monitor = `${POCKET_SCOPE} .stage__monitor`;

    // The height of the box, which the sheets open under.
    expect(
      declaration(
        pocketStyles,
        `:root:has(${playing}[data-dock="true"])`,
        "--pocket-picture-h",
      ),
    ).toBe("var(--pocket-pinned-h, var(--pocket-band-h))");
    expect(
      declaration(
        pocketStyles,
        `:root:has(${playing}[data-dock="false"])`,
        "--pocket-picture-h",
      ),
    ).toBe("var(--pocket-pinned-h, var(--pocket-monitor-h))");
    expect(
      declaration(pocketStyles, `${STACKED_POCKET} .stage__monitor`, "height"),
    ).toBe("var(--pocket-picture-h)");
    // Its width, across the window and in the band.
    expect(declaration(pocketStyles, monitor, "width")).toBe(
      "var(--pocket-pinned-w, auto)",
    );
    expect(
      declaration(
        pocketStyles,
        `${STACKED_POCKET}[data-dock="true"] .stage__monitor`,
        "width",
      ),
    ).toBe("var(--pocket-pinned-w, var(--pocket-band-w))");
    expect(
      declaration(
        pocketStyles,
        `${STACKED_POCKET}[data-mode="guide"][data-dock="true"] .stage`,
        "grid-template-columns",
      ),
    ).toBe("var(--pocket-pinned-w, var(--pocket-band-w)) minmax(0, 1fr)");
    // A short landscape window sizes the box by its column and its row.
    expect(
      declaration(
        pocketStyles,
        `${SPLIT_POCKET}[data-playing="true"] .shell__workspace`,
        "grid-template-columns",
      ),
    ).toBe("minmax(var(--pocket-pinned-w, 0px), 45fr) minmax(0, 55fr)");
    expect(
      declaration(
        pocketStyles,
        `${SPLIT_POCKET} .stage`,
        "grid-template-rows",
      ),
    ).toBe("var(--pocket-pinned-h, minmax(0, 1fr)) auto");
  });

  it("starts pocket's load notice at the top where its box is too low for it", async () => {
    // Centred content that overflows is cut at both ends, and the cut top
    // cannot be scrolled to. The notice is centred by auto margins, which
    // give way first, and the band shows its heading alone from the top left.
    const pocketStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      POCKET_STYLESHEET,
    );
    const notice = `${POCKET_SCOPE} .playback-load-notice`;
    const docked = `${STACKED_POCKET}[data-dock="true"] .playback-load-notice`;

    expect(declaration(pocketStyles, notice, "display")).toBe("flex");
    expect(declaration(pocketStyles, notice, "place-content")).toBe("normal");
    expect(declaration(pocketStyles, notice, "flex-direction")).toBe("column");
    expect(declaration(pocketStyles, `${notice} h2`, "margin-top")).toBe("auto");
    expect(
      declaration(pocketStyles, `${notice}__actions`, "margin-bottom"),
    ).toBe("auto");
    expect(
      declaration(pocketStyles, docked, "align-items"),
    ).toBe("start");
    expect(
      declaration(
        pocketStyles,
        `${docked} h2`,
        "margin-top",
      ),
    ).toBe("0");
    expect(
      declaration(
        pocketStyles,
        `${docked} p, ${docked}__actions`,
        "display",
      ),
    ).toBe("none");
  });

  it("keeps what the TV draws clear of its docked picture's box, placed by the size it is held at", async () => {
    // Media3 draws the TV's docked picture above the page. A veil over the
    // Guide or a sheet's backdrop must leave its box out, and the panel and
    // the board beside and under it are placed by the held size, so a
    // narrower window moves nothing under a picture that stays where it is.
    const tvStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      "features/stage/television.css",
    );
    const playing = '.shell[data-layout="pocket"][data-playing="true"]';

    expect(declaration(tvStyles, "html[data-television]", "--tv-panel-left")).toBe(
      "calc(var(--tv-gutter) + var(--pocket-pinned-w, var(--tv-dock-w)) + 1.8333rem)",
    );
    expect(declaration(tvStyles, "html[data-television]", "--tv-band-h")).toContain(
      "var(--pocket-pinned-h, var(--tv-dock-h))",
    );
    expect(
      declaration(
        tvStyles,
        `html[data-television]:has(.board-search input[aria-expanded="true"]) ${playing} .programme-guide::after`,
        "clip-path",
      ),
    ).toBe("var(--tv-around-dock)");
    expect(
      declaration(
        tvStyles,
        `html[data-television]:has(${playing}) :is(.board-search-desk__backdrop, .group-roster__backdrop, .feeds-dialog__backdrop)`,
        "clip-path",
      ),
    ).toBe("var(--tv-around-dock)");
  });

  it("puts the TV's opening-signal message and actions in the fullscreen band, under the picture's box", async () => {
    // Media3's opaque native view may lie over the box before the stream
    // prepares; what the viewer reads and presses then must be outside it.
    const tvStyles = requireStylesheet(
      new Map(await installedStylesheets()),
      "features/stage/television.css",
    );
    const opening =
      'html[data-television] .shell[data-layout="pocket"] .hosted-player:fullscreen:is([data-player-state="starting"], [data-player-state="stopping"])';

    expect(declaration(tvStyles, `${opening} .hosted-player__controls`, "top")).toBeUndefined();
    expect(declaration(tvStyles, `${opening} .hosted-player__overlay`, "top")).toBe(
      "calc(100vh - var(--fs-row-b-bottom) - var(--tv-control))",
    );
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
    // A value written over several lines reads as one.
    declarations.set(
      declaration.prop,
      declaration.value.trim().replace(/\s+/gu, " "),
    );
  });
  return declarations;
}

function declaration(
  stylesheet: postcss.Root,
  selector: string,
  property: string,
): string | undefined {
  let value: string | undefined;
  stylesheet.walkRules((rule) => {
    if (rule.parent?.type !== "root" || rule.selector.replace(/\s+/gu, " ") !== selector) return;
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
