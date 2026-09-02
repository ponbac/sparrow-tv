import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { ChannelGroup } from "../../client/contracts";
import { groupDisplayName, visibleChannelGroups } from "./board-group-roster";
import { GroupRosterDialog } from "./group-roster-dialog";

const ALL_GROUPS = "all";
const GROUP_PREFIX = "group:";

/** Inputs for the scrollable Channel Group lane and roster. */
export interface ChannelGroupLaneProps {
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly excluded: ReadonlySet<string>;
  readonly onSelectGroup: (group: string | null) => void;
  readonly onPrefetchGroup: (group: string | null) => void;
  readonly onSetExcluded: (name: string, exclude: boolean) => void;
  readonly onRestoreAll: () => void;
}

/**
 * Renders Channel Groups as a horizontally browsable lane with overflow
 * steppers and a roster for jumping or excluding groups.
 */
export function ChannelGroupLane({
  groups,
  activeGroup,
  excluded,
  onSelectGroup,
  onPrefetchGroup,
  onSetExcluded,
  onRestoreAll,
}: ChannelGroupLaneProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const visibleGroups = visibleChannelGroups(groups, excluded);
  const overflow = useLaneOverflow(scrollerRef, visibleGroups.length);
  const activeFilter = filterValue(activeGroup);

  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller === null) {
      return;
    }
    const checked = scroller.querySelector<HTMLElement>("[data-checked]");
    if (checked === null) {
      return;
    }
    scrollChildIntoLane(scroller, checked);
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
    <div
      className="programme-guide__lane"
      data-overflow={overflow.canBack || overflow.canAhead ? "true" : "false"}
      data-can-back={overflow.canBack ? "true" : "false"}
      data-can-ahead={overflow.canAhead ? "true" : "false"}
    >
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
          onValueChange={(value) => onSelectGroup(groupFromFilterValue(value))}
          aria-label="Channel groups"
        >
          <Radio.Root
            className="programme-guide__group"
            data-acceptance-group
            value={ALL_GROUPS}
            onMouseEnter={() => onPrefetchGroup(null)}
            onFocus={() => onPrefetchGroup(null)}
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
                onFocus={() => onPrefetchGroup(group.name)}
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
        groups={groups}
        activeGroup={activeGroup}
        excluded={excluded}
        onSelectGroup={onSelectGroup}
        onPrefetchGroup={onPrefetchGroup}
        onSetExcluded={onSetExcluded}
        onRestoreAll={onRestoreAll}
      />
    </div>
  );
}

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

    measure();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(node);
    node.addEventListener("scroll", measure, { passive: true });
    node.addEventListener("wheel", onLaneWheel, { passive: false });
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      node.removeEventListener("scroll", measure);
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

function scrollChildIntoLane(scroller: HTMLElement, child: HTMLElement): void {
  const scrollerRect = scroller.getBoundingClientRect();
  const childRect = child.getBoundingClientRect();
  if (childRect.left < scrollerRect.left) {
    scroller.scrollLeft -= scrollerRect.left - childRect.left + 8;
    return;
  }
  if (childRect.right > scrollerRect.right) {
    scroller.scrollLeft += childRect.right - scrollerRect.right + 8;
  }
}

function filterValue(group: string | null): string {
  return group === null ? ALL_GROUPS : `${GROUP_PREFIX}${group}`;
}

function groupFromFilterValue(value: string): string | null {
  return value === ALL_GROUPS ? null : value.slice(GROUP_PREFIX.length);
}
