import { memo, useState } from "react";
import type { ChannelIcon as ChannelIconLocation } from "../../client/contracts";

type IconStatus = "none" | "loading" | "loaded" | "failed";

/**
 * A Channel's tile on the TV: its Channel Icon on a plate, or its Channel
 * Number when it has no icon or the icon does not load. The tile keeps one
 * size in every state, so an icon that arrives moves nothing, and a broken
 * one never shows the browser's broken-image glyph: the image is shown only
 * once it has loaded and leaves the tile when it fails.
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
  const [settled, setSettled] = useState<{
    readonly icon: ChannelIconLocation;
    readonly status: "loaded" | "failed";
  } | null>(null);
  const status: IconStatus =
    icon === null
      ? "none"
      : settled !== null && settled.icon === icon
        ? settled.status
        : "loading";
  return (
    <span
      className={className === undefined ? "channel-icon" : `channel-icon ${className}`}
      data-icon={status}
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
          onLoad={() => setSettled({ icon, status: "loaded" })}
          onError={() => setSettled({ icon, status: "failed" })}
        />
      )}
    </span>
  );
});
