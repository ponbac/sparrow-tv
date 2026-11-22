import type { ChannelSummary } from "../../client/contracts";
import { qualityLabel } from "./guide-families";

/**
 * The quality tags of a folded TV result: one per matched Quality Variant,
 * the one the result tunes marked.
 */
export function SearchQualities({
  variants,
  current,
}: {
  readonly variants: readonly ChannelSummary[];
  readonly current: ChannelSummary;
}) {
  const tagged = variants.filter((variant) => variant.variant !== null);
  if (tagged.length === 0) return null;
  return (
    <span className="board-search__qualities">
      {tagged.map((variant) =>
        variant.variant === null ? null : (
          <span
            key={variant.id}
            className="board-search__quality"
            data-current={tagged.length > 1 && variant.id === current.id}
          >
            {qualityLabel(variant.variant.quality)}
          </span>
        ),
      )}
    </span>
  );
}
