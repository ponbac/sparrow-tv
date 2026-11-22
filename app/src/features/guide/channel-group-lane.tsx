import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { CalendarRange, ChevronLeft, ChevronRight, Clock, LayoutGrid } from "lucide-react";
import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type RefObject,
} from "react";
import type { ChannelGroup } from "../../client/contracts";
import { groupDisplayName, visibleChannelGroups } from "./board-group-roster";
import {
  revealInHorizontalScroller,
  revealInScrollContainer,
} from "../stage/stage-dom";
import { GroupRosterDialog } from "./group-roster-dialog";
import { clockLabel } from "./guide-window";
import { railFit, railScrollTop } from "./rail-fit";

const ALL_GROUPS = "all";
const GROUP_PREFIX = "group:";
const FOCUS_PREFETCH_DEBOUNCE_MS = 150;
// Keep enough real chips to fill the initial TV lane. Browsing the lane extends
// it before focus reaches the end; the roster always contains every group.
const TV_GROUP_BATCH = 24;

/** The time the now-and-next list looks at, and the chips that change it. */
export interface GuideTime {
  /** The row of times is shown. */
  readonly open: boolean;
  /** The time the list looks at, or null for now. */
  readonly chosen: Date | null;
  /** The later times on offer, earliest first. */
  readonly options: readonly Date[];
  readonly onToggle: () => void;
  readonly onChoose: (time: Date | null) => void;
}

/** The TV Guide's two layouts and the chip at the head of the lane that swaps them. */
export interface GuideView {
  readonly current: "now" | "timeline";
  readonly onToggle: () => void;
}

/** Inputs for the scrollable Channel Group lane and roster. */
export interface ChannelGroupLaneProps {
  /** Decouples D-pad arrow focus from group selection until Enter/Space on TV. */
  readonly television?: boolean;
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  /** Null where the guide is a timeline, which shows every time at once. */
  readonly time: GuideTime | null;
  /** The TV Guide's layout switch, or null where there is one layout. */
  readonly view?: GuideView | null;
  /** The TV timeline stands the lane on its side as a rail of groups. */
  readonly orientation?: "horizontal" | "vertical";
  readonly excluded: ReadonlySet<string>;
  readonly onSelectGroup: (group: string | null) => void;
  readonly onPrefetchGroup: (group: string | null) => void;
  readonly onSetExcluded: (name: string, exclude: boolean) => void;
  readonly onRestoreAll: () => void;
}

/**
 * Renders Channel Groups as a horizontally browsable lane with overflow
 * steppers and a roster for jumping or excluding groups. Given a time, the
 * lane starts with a chip that opens a row of times under it.
 */
export const ChannelGroupLane = memo(function ChannelGroupLane({
  television = false,
  groups,
  activeGroup,
  time,
  view = null,
  orientation = "horizontal",
  excluded,
  onSelectGroup,
  onPrefetchGroup,
  onSetExcluded,
  onRestoreAll,
}: ChannelGroupLaneProps) {
  const vertical = orientation === "vertical";
  const scrollerRef = useRef<HTMLDivElement>(null);
  const focusPrefetchTimerRef = useRef<number | null>(null);
  const timesId = useId();
  const visibleGroups = useMemo(
    () => visibleChannelGroups(groups, excluded),
    [groups, excluded],
  );
  const [groupLimit, setGroupLimit] = useState(TV_GROUP_BATCH);
  const selectedIndex = activeGroup === null
    ? -1
    : visibleGroups.findIndex((group) => group.name === activeGroup);
  const shownGroups = television
    ? visibleGroups.slice(0, Math.max(groupLimit, selectedIndex + TV_GROUP_BATCH))
    : visibleGroups;
  const hasMoreGroups = shownGroups.length < visibleGroups.length;
  const extendLane = useCallback(() => {
    setGroupLimit(shownGroups.length + TV_GROUP_BATCH);
  }, [shownGroups.length]);
  const overflow = useLaneOverflow(
    scrollerRef, shownGroups.length, vertical, hasMoreGroups ? extendLane : null,
  );
  useRailFit(scrollerRef, vertical, shownGroups.length);
  const activeFilter = filterValue(activeGroup);

  const cancelFocusPrefetch = useCallback(() => {
    if (focusPrefetchTimerRef.current !== null) {
      window.clearTimeout(focusPrefetchTimerRef.current);
      focusPrefetchTimerRef.current = null;
    }
  }, []);

  const scheduleFocusPrefetch = useCallback(
    (group: string | null) => {
      cancelFocusPrefetch();
      focusPrefetchTimerRef.current = window.setTimeout(() => {
        focusPrefetchTimerRef.current = null;
        onPrefetchGroup(group);
      }, FOCUS_PREFETCH_DEBOUNCE_MS);
    },
    [cancelFocusPrefetch, onPrefetchGroup],
  );

  useEffect(() => cancelFocusPrefetch, [cancelFocusPrefetch]);

  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller === null) {
      return;
    }
    const checked = scroller.querySelector<HTMLElement>("[data-checked]");
    if (checked === null) {
      return;
    }
    if (vertical) {
      revealRailRow(scroller, checked);
    } else {
      revealInHorizontalScroller(scroller, checked);
    }
  }, [activeGroup, vertical, visibleGroups.length]);
  const reveal = (target: HTMLElement) => {
    if (vertical) {
      revealRailRow(scrollerRef.current, target);
    } else {
      revealInHorizontalScroller(scrollerRef.current, target);
    }
  };

  const step = (direction: -1 | 1) => {
    const scroller = scrollerRef.current;
    if (scroller === null) {
      return;
    }
    const distance = Math.max(120, scroller.clientWidth * 0.7) * direction;
    scroller.scrollLeft += distance;
  };

  const groupRadio = (group: ChannelGroup | null, index: number) => {
    const name = group?.name ?? null;
    const value = filterValue(name);
    const props = {
      className: "programme-guide__group",
      "data-acceptance-group": true,
      onMouseEnter: () => onPrefetchGroup(name),
      onFocus: (event: FocusEvent<HTMLElement>) => {
        if (hasMoreGroups && index >= shownGroups.length - 4) extendLane();
        reveal(event.currentTarget);
        scheduleFocusPrefetch(name);
      },
      onBlur: cancelFocusPrefetch,
    };
    const label = group === null ? "All" : (
      <>{groupDisplayName(group.name)}<em>{group.channelCount}</em></>
    );
    // The Guide already owns TV arrow navigation. A plain button lets focus
    // browse independently of selection without registering every group with
    // a second keyboard controller during the first Guide render.
    if (television) {
      const checked = name === activeGroup;
      return (
        <button
          key={value}
          {...props}
          type="button"
          role="radio"
          aria-setsize={visibleGroups.length + 1}
          aria-posinset={index + 2}
          aria-checked={checked}
          data-checked={checked ? "" : undefined}
          tabIndex={checked ? 0 : -1}
          onClick={() => onSelectGroup(name)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              event.stopPropagation();
              onSelectGroup(name);
            }
          }}
        >
          {label}
        </button>
      );
    }
    return <Radio.Root key={value} {...props} value={value}>{label}</Radio.Root>;
  };
  const radios = [groupRadio(null, -1), ...shownGroups.map(groupRadio)];

  return (
    <>
      <div
        className="programme-guide__lane"
        data-overflow={overflow.canBack || overflow.canAhead ? "true" : "false"}
        data-can-back={overflow.canBack ? "true" : "false"}
        data-can-ahead={overflow.canAhead ? "true" : "false"}
        data-orientation={orientation}
      >
        {view === null ? null : (
          <button
            className="programme-guide__view"
            type="button"
            aria-label="Timeline"
            aria-pressed={view.current === "timeline"}
            onClick={view.onToggle}
          >
            <span data-on={view.current === "now"}>
              <LayoutGrid aria-hidden="true" />
              Now
            </span>
            <span data-on={view.current === "timeline"}>
              <CalendarRange aria-hidden="true" />
              Timeline
            </span>
          </button>
        )}
        {time === null ? null : (
          <button
            className="programme-guide__when"
            type="button"
            aria-controls={time.open ? timesId : undefined}
            aria-expanded={time.open}
            aria-pressed={time.chosen !== null}
            onClick={time.onToggle}
          >
            <Clock aria-hidden="true" />
            {time.chosen === null ? "Now" : clockLabel(time.chosen)}
          </button>
        )}
        <button
          className="programme-guide__lane-step"
          type="button"
          aria-label="Earlier Channel Groups"
          disabled={!overflow.canBack}
          onClick={() => step(-1)}
        >
          <ChevronLeft aria-hidden="true" />
        </button>
        <div className="programme-guide__groups-scroller" ref={scrollerRef}>
          {television ? (
            <div
              className="programme-guide__groups"
              role="radiogroup"
              aria-label="Channel groups"
              aria-orientation={orientation}
            >
              {radios}
            </div>
          ) : (
            <RadioGroup
              className="programme-guide__groups"
              value={activeFilter}
              onValueChange={(value) => onSelectGroup(groupFromFilterValue(value))}
              aria-label="Channel groups"
            >
              {radios}
            </RadioGroup>
          )}
        </div>
        <button
          className="programme-guide__lane-step"
          type="button"
          aria-label="Later Channel Groups"
          disabled={!overflow.canAhead}
          onClick={() => step(1)}
        >
          <ChevronRight aria-hidden="true" />
        </button>
        <GroupRosterDialog
          television={television}
          groups={groups}
          activeGroup={activeGroup}
          excluded={excluded}
          onSelectGroup={onSelectGroup}
          onPrefetchGroup={onPrefetchGroup}
          onSetExcluded={onSetExcluded}
          onRestoreAll={onRestoreAll}
        />
      </div>
      {time?.open === true ? (
        // Not Channel Groups: these stay out of the lane's radio group.
        <div
          className="programme-guide__times"
          id={timesId}
          role="group"
          aria-label="Time"
        >
          <button
            type="button"
            aria-pressed={time.chosen === null}
            onClick={() => time.onChoose(null)}
          >
            Now
          </button>
          {time.options.map((option) => (
            <button
              key={option.getTime()}
              type="button"
              aria-pressed={time.chosen?.getTime() === option.getTime()}
              onClick={() => time.onChoose(option)}
            >
              {clockLabel(option)}
            </button>
          ))}
        </div>
      ) : null}
    </>
  );
});

function useLaneOverflow(
  scrollerRef: RefObject<HTMLDivElement | null>,
  itemCount: number,
  vertical: boolean,
  extend: (() => void) | null,
): { readonly canBack: boolean; readonly canAhead: boolean } {
  const [overflow, setOverflow] = useState({
    canBack: false,
    canAhead: false,
  });

  useLayoutEffect(() => {
    const node = scrollerRef.current;
    if (node === null) {
      return;
    }

    let scrollFrame: number | null = null;
    const measure = () => {
      const max = Math.max(0, node.scrollWidth - node.clientWidth);
      if (extend !== null) {
        const viewport = vertical ? node.clientHeight : node.clientWidth;
        const remaining = vertical ? node.scrollHeight - node.scrollTop - viewport : max - node.scrollLeft;
        if (viewport > 0 && remaining < viewport) extend();
      }
      const canBack = node.scrollLeft > 1;
      const canAhead = max - node.scrollLeft > 1;
      setOverflow((current) =>
        current.canBack === canBack && current.canAhead === canAhead
          ? current
          : { canBack, canAhead },
      );
    };
    const onScroll = () => {
      if (scrollFrame !== null) {
        return;
      }
      scrollFrame = window.requestAnimationFrame(() => {
        scrollFrame = null;
        measure();
      });
    };

    measure();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(node);
    node.addEventListener("scroll", onScroll, { passive: true });
    node.addEventListener("wheel", onLaneWheel, { passive: false });
    window.addEventListener("resize", measure);
    return () => {
      if (scrollFrame !== null) {
        window.cancelAnimationFrame(scrollFrame);
        scrollFrame = null;
      }
      observer?.disconnect();
      node.removeEventListener("scroll", onScroll);
      node.removeEventListener("wheel", onLaneWheel);
      window.removeEventListener("resize", measure);
    };
  }, [extend, itemCount, scrollerRef, vertical]);

  return overflow;
}

/** The rail's rows, and how far apart they lie. */
function railRows(
  scroller: HTMLElement,
): { readonly rows: HTMLElement[]; readonly height: number; readonly pitch: number } | null {
  const rows = Array.from(
    scroller.querySelectorAll<HTMLElement>(".programme-guide__group"),
  );
  const first = rows[0]?.getBoundingClientRect();
  if (first === undefined || first.height <= 0) return null;
  const second = rows[1]?.getBoundingClientRect();
  const pitch = second === undefined ? first.height : second.top - first.top;
  return pitch >= first.height ? { rows, height: first.height, pitch } : null;
}

/**
 * Keeps a vertical rail's view on whole rows: the room under the last whole
 * row goes to `--rail-rest`, which the TV covers, so no group is ever half
 * shown or faded, and `data-more` says whether groups lie below. Measured
 * again whenever the rail changes size or scrolls.
 */
function useRailFit(
  scrollerRef: RefObject<HTMLDivElement | null>,
  enabled: boolean,
  itemCount: number,
): void {
  useLayoutEffect(() => {
    const node = scrollerRef.current;
    if (node === null || !enabled) {
      return;
    }
    const measure = () => {
      const rail = railRows(node);
      const rest =
        rail === null ? 0 : railFit(node.clientHeight, rail.height, rail.pitch).rest;
      node.style.setProperty("--rail-rest", `${rest}px`);
      markMore();
    };
    // Whether groups lie below the view, for the cue in the covered room.
    const markMore = () => {
      const more = node.scrollTop + node.clientHeight < node.scrollHeight - 1;
      if (node.dataset.more !== String(more)) {
        node.dataset.more = String(more);
      }
    };
    measure();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(node);
    node.addEventListener("scroll", markMore, { passive: true });
    return () => {
      observer?.disconnect();
      node.removeEventListener("scroll", markMore);
      node.style.removeProperty("--rail-rest");
      delete node.dataset.more;
    };
  }, [enabled, itemCount, scrollerRef]);
}

/** Scrolls a vertical rail by whole rows until `target` is wholly in view. */
function revealRailRow(scroller: HTMLElement | null, target: HTMLElement): void {
  if (scroller === null || scroller.clientHeight === 0) {
    return;
  }
  const rail = railRows(scroller);
  const index = rail?.rows.indexOf(target) ?? -1;
  if (rail === null || index === -1) {
    revealInScrollContainer(scroller, target);
    return;
  }
  const next = railScrollTop(
    index,
    rail.rows.length,
    scroller.scrollTop,
    railFit(scroller.clientHeight, rail.height, rail.pitch),
    rail.pitch,
  );
  if (next !== scroller.scrollTop) {
    scroller.scrollTop = next;
  }
}

function onLaneWheel(event: WheelEvent): void {
  if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) {
    return;
  }
  const node = event.currentTarget;
  if (!(node instanceof HTMLElement)) {
    return;
  }
  const max = node.scrollWidth - node.clientWidth;
  if (max <= 0) {
    return;
  }
  const next = Math.min(max, Math.max(0, node.scrollLeft + event.deltaY));
  if (next === node.scrollLeft) {
    return;
  }
  event.preventDefault();
  node.scrollLeft = next;
}

function filterValue(group: string | null): string {
  return group === null ? ALL_GROUPS : `${GROUP_PREFIX}${group}`;
}

function groupFromFilterValue(value: string): string | null {
  return value === ALL_GROUPS ? null : value.slice(GROUP_PREFIX.length);
}
