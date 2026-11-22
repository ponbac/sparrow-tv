import { memo, useState } from "react";
import type { ChannelIcon as ChannelIconLocation } from "../../client/contracts";
import { iconShape, type IconShape } from "./icon-shape";

type IconStatus = "none" | "loading" | "loaded" | "failed";

/**
 * A Channel's tile on the TV: its Channel Icon on a plate, or its Channel
 * Number when it has no icon or the icon does not load. The tile keeps one
 * size in every state, so an icon that arrives moves nothing, and a broken
 * one never shows the browser's broken-image glyph: the image is shown only
 * once it has loaded and leaves the tile when it fails. A loaded icon also
 * says its outline (`data-shape`), so the plate can fit it.
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
      }
    | { readonly icon: ChannelIconLocation; readonly status: "failed" }
    | null
  >(null);
  const current = icon !== null && settled !== null && settled.icon === icon ? settled : null;
  const status: IconStatus =
    icon === null ? "none" : current === null ? "loading" : current.status;
  const shape = current?.status === "loaded" ? current.shape : null;
  return (
    <span
      className={className === undefined ? "channel-icon" : `channel-icon ${className}`}
      data-icon={status}
      data-shape={shape ?? undefined}
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
          onLoad={(event) =>
            setSettled({
              icon,
              status: "loaded",
              shape: iconShape(
                event.currentTarget.naturalWidth,
                event.currentTarget.naturalHeight,
              ),
            })
          }
          onError={() => setSettled({ icon, status: "failed" })}
        />
      )}
    </span>
  );
});
