import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { ChevronLeft, ChevronRight, Clock } from "lucide-react";
import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import type { ChannelGroup } from "../../client/contracts";
import { groupDisplayName, visibleChannelGroups } from "./board-group-roster";
import { revealInHorizontalScroller } from "../stage/stage-dom";
import { GroupRosterDialog } from "./group-roster-dialog";
import { clockLabel } from "./guide-window";

const ALL_GROUPS = "all";
const GROUP_PREFIX = "group:";
const FOCUS_PREFETCH_DEBOUNCE_MS = 150;

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

/** Inputs for the scrollable Channel Group lane and roster. */
export interface ChannelGroupLaneProps {
  /** Decouples D-pad arrow focus from group selection until Enter/Space on TV. */
  readonly television?: boolean;
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  /** Null where the guide is a timeline, which shows every time at once. */
  readonly time: GuideTime | null;
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
  excluded,
  onSelectGroup,
  onPrefetchGroup,
  onSetExcluded,
  onRestoreAll,
}: ChannelGroupLaneProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const focusPrefetchTimerRef = useRef<number | null>(null);
  const timesId = useId();
  const visibleGroups = useMemo(
    () => visibleChannelGroups(groups, excluded),
    [groups, excluded],
  );
  const overflow = useLaneOverflow(scrollerRef, visibleGroups.length);
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
    revealInHorizontalScroller(scroller, checked);
  }, [activeGroup, visibleGroups.length]);

  const step = (direction: -1 | 1) => {
    const scroller = scrollerRef.current;
    if (scroller === null) {
      return;
    }
    const distance = Math.max(120, scroller.clientWidth * 0.7) * direction;
    scroller.scrollLeft += distance;
  };

  return (
    <>
      <div
        className="programme-guide__lane"
        data-overflow={overflow.canBack || overflow.canAhead ? "true" : "false"}
        data-can-back={overflow.canBack ? "true" : "false"}
        data-can-ahead={overflow.canAhead ? "true" : "false"}
      >
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
          <RadioGroup
            className="programme-guide__groups"
            value={activeFilter}
            onKeyDownCapture={(event) => {
              // On TV, stop Base UI RadioGroup from arming focus-follows-selection
              // on arrow keys while preserving pointer clicks and Enter/Space commit.
              if (television && event.key.startsWith("Arrow")) {
                event.preventBaseUIHandler();
              }
            }}
            onValueChange={(value) => onSelectGroup(groupFromFilterValue(value))}
            aria-label="Channel groups"
          >
            <Radio.Root
              className="programme-guide__group"
              data-acceptance-group
              value={ALL_GROUPS}
              onMouseEnter={() => onPrefetchGroup(null)}
              onFocus={(event) => {
                revealInHorizontalScroller(scrollerRef.current, event.currentTarget);
                scheduleFocusPrefetch(null);
              }}
              onBlur={cancelFocusPrefetch}
              onKeyDown={(event) => {
                if (television && (event.key === "Enter" || event.key === " ")) {
                  event.preventDefault();
                  event.stopPropagation();
                  onSelectGroup(null);
                }
              }}
            >
              All
            </Radio.Root>
            {visibleGroups.map((group) => {
              const groupValue = filterValue(group.name);
              return (
                <Radio.Root
                  className="programme-guide__group"
                  data-acceptance-group
                  key={groupValue}
                  value={groupValue}
                  onMouseEnter={() => onPrefetchGroup(group.name)}
                  onFocus={(event) => {
                    revealInHorizontalScroller(
                      scrollerRef.current,
                      event.currentTarget,
                    );
                    scheduleFocusPrefetch(group.name);
                  }}
                  onBlur={cancelFocusPrefetch}
                  onKeyDown={(event) => {
                    if (television && (event.key === "Enter" || event.key === " ")) {
                      event.preventDefault();
                      event.stopPropagation();
                      onSelectGroup(group.name);
                    }
                  }}
                >
                  {groupDisplayName(group.name)}
                  <em>{group.channelCount}</em>
                </Radio.Root>
              );
            })}
          </RadioGroup>
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
  }, [itemCount, scrollerRef]);

  return overflow;
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
