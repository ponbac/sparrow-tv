import { memo, useState, type CSSProperties } from "react";
import type { ChannelIcon as ChannelIconLocation } from "../../client/contracts";
import { iconFit, loadedIconInk, type IconFit, type IconInk } from "./icon-fit";
import { iconShape, type IconShape } from "./icon-shape";

type IconStatus = "none" | "loading" | "loaded" | "failed";

/**
 * A Channel's tile on the TV: its Channel Icon on a plate, or its Channel
 * Number when it has no icon or the icon does not load. The tile keeps one
 * size in every state, so an icon that arrives moves nothing, and a broken
 * one never shows the browser's broken-image glyph: the image is shown only
 * once it has loaded and leaves the tile when it fails. A loaded icon also
 * says its outline (`data-shape`), so the plate can fit it. On the TV, where
 * its pixels can be read, it is fitted by its ink instead (`data-fit`, see
 * `icon-fit.ts`), on a plate in its tile's own ground.
 *
 * The icon's location is never logged or reported; the WebView fetches it
 * without a referrer.
 */
export const ChannelIcon = memo(function ChannelIcon({
  icon,
  number,
  className,
}: {
  readonly icon: ChannelIconLocation | null;
  readonly number: number;
  readonly className?: string;
}) {
  // Keyed by location, so a row that shows another Channel starts over.
  const [settled, setSettled] = useState<
    | {
        readonly icon: ChannelIconLocation;
        readonly status: "loaded";
        readonly shape: IconShape | null;
        readonly fit: IconFit | null;
      }
    | { readonly icon: ChannelIconLocation; readonly status: "failed" }
    | null
  >(null);
  const current = icon !== null && settled !== null && settled.icon === icon ? settled : null;
  const status: IconStatus =
    icon === null ? "none" : current === null ? "loading" : current.status;
  const shape = current?.status === "loaded" ? current.shape : null;
  const fit = current?.status === "loaded" ? current.fit : null;
  return (
    <span
      className={className === undefined ? "channel-icon" : `channel-icon ${className}`}
      data-icon={status}
      data-shape={shape ?? undefined}
      data-fit={fit === null ? undefined : "ink"}
      data-ground={fit?.ground == null ? undefined : true}
      style={fit === null ? undefined : fitStyle(fit)}
      aria-hidden="true"
    >
      <span className="channel-icon__number">{number}</span>
      {icon === null || status === "failed" ? null : (
        <img
          key={icon}
          className="channel-icon__image"
          src={icon}
          alt=""
          loading="lazy"
          decoding="async"
          draggable={false}
          referrerPolicy="no-referrer"
          onLoad={(event) => {
            const image = event.currentTarget;
            const { naturalWidth, naturalHeight } = image;
            const fitted = (ink: IconInk | null) =>
              ink === null ? null : iconFit(ink, naturalWidth, naturalHeight);
            const ink = televisionShown() ? loadedIconInk(icon, image) : null;
            setSettled({
              icon,
              status: "loaded",
              shape: iconShape(naturalWidth, naturalHeight),
              fit: ink instanceof Promise ? null : fitted(ink),
            });
            if (ink instanceof Promise) {
              void ink.then((read) => {
                const later = fitted(read);
                if (later === null) return;
                setSettled((now) =>
                  now !== null && now.icon === icon && now.status === "loaded"
                    ? { ...now, fit: later }
                    : now,
                );
              });
            }
          }}
          onError={() => setSettled({ icon, status: "failed" })}
        />
      )}
    </span>
  );
});

/** Whether the TV's styles are in force, the only place an ink fit shows. */
function televisionShown(): boolean {
  return document.documentElement.hasAttribute("data-television");
}

/** The fit as the custom properties `television.css` places the image by. */
function fitStyle(fit: IconFit): CSSProperties {
  const percent = (fraction: number) => `${(fraction * 100).toFixed(3)}%`;
  return {
    "--fit-left": percent(fit.left),
    "--fit-top": percent(fit.top),
    "--fit-width": percent(fit.width),
    "--fit-height": percent(fit.height),
    "--fit-clip": fit.clip ?? "none",
    ...(fit.ground === null ? {} : { "--fit-ground": fit.ground }),
  } as CSSProperties;
}
