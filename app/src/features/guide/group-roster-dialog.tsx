import { Dialog } from "@base-ui/react/dialog";
import { Eye, EyeOff, ListFilter, Search, X } from "lucide-react";
import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { ChannelGroup } from "../../client/contracts";
import { groupDisplayName } from "./board-group-roster";
import {
  handleLinearDialogArrows,
  isVisibleDialogControl,
  revealInScrollContainer,
  useTelevisionInputEdit,
} from "./dialog-navigation";
import "./group-roster-dialog.css";

/** Inputs for the Channel Group list where groups are picked or hidden. */
export interface GroupRosterDialogProps {
  /** Focuses the active group row on open and keeps search press-to-edit on TV. */
  readonly television?: boolean;
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly excluded: ReadonlySet<string>;
  readonly onSelectGroup: (group: string | null) => void;
  readonly onPrefetchGroup: (group: string | null) => void;
  readonly onSetExcluded: (name: string, exclude: boolean) => void;
  readonly onRestoreAll: () => void;
}

/**
 * Opens a searchable Channel Group roster over the guide pane so the viewer can
 * jump to a group or hide it from the guide.
 */
export function GroupRosterDialog({
  television = false,
  groups,
  activeGroup,
  excluded,
  onSelectGroup,
  onPrefetchGroup,
  onSetExcluded,
  onRestoreAll,
}: GroupRosterDialogProps) {
  const [query, setQuery] = useState("");
  const popupRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchEdit = useTelevisionInputEdit(television);
  const matches = useMemo(
    () => groups.filter((group) => groupMatches(group, query)),
    [groups, query],
  );
  const excludedCount = excluded.size;
  const visibleCount = Math.max(0, groups.length - excludedCount);

  const handlePopupKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const popup = popupRef.current;
    const list = listRef.current;
    if (
      !television ||
      popup === null ||
      event.defaultPrevented ||
      event.shiftKey ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.nativeEvent.isComposing ||
      !(event.target instanceof HTMLElement)
    ) {
      return;
    }
    const { key, target } = event;
    if (
      key !== "ArrowUp" &&
      key !== "ArrowDown" &&
      key !== "ArrowLeft" &&
      key !== "ArrowRight"
    ) {
      return;
    }
    const row = target.closest<HTMLElement>(".group-roster__row");
    if (row !== null && list !== null && list.contains(row)) {
      const isExclude = target.classList.contains("group-roster__exclude");
      if (key === "ArrowRight") {
        event.preventDefault();
        event.stopPropagation();
        if (!isExclude) {
          const sibling = row.querySelector<HTMLElement>(".group-roster__exclude");
          if (sibling !== null && isVisibleDialogControl(sibling)) {
            sibling.focus({ preventScroll: true });
            revealInScrollContainer(list, sibling);
          }
        }
        return;
      }
      if (key === "ArrowLeft") {
        event.preventDefault();
        event.stopPropagation();
        if (isExclude) {
          const sibling = row.querySelector<HTMLElement>(".group-roster__pick");
          if (sibling !== null && isVisibleDialogControl(sibling)) {
            sibling.focus({ preventScroll: true });
            revealInScrollContainer(list, sibling);
          }
        }
        return;
      }
      const rows = Array.from(
        list.querySelectorAll<HTMLElement>(".group-roster__row"),
      );
      const rowIndex = rows.indexOf(row);
      if (rowIndex === -1) return;
      event.preventDefault();
      event.stopPropagation();
      const nextRowIndex = key === "ArrowDown" ? rowIndex + 1 : rowIndex - 1;
      if (key === "ArrowUp" && isExclude) {
        const prevExclude =
          nextRowIndex >= 0
            ? (rows[nextRowIndex]?.querySelector<HTMLElement>(
                ".group-roster__exclude",
              ) ?? null)
            : null;
        if (prevExclude !== null && isVisibleDialogControl(prevExclude)) {
          prevExclude.focus({ preventScroll: true });
          revealInScrollContainer(list, prevExclude);
        } else {
          const restore = popup.querySelector<HTMLElement>(
            ".group-roster__restore",
          );
          const toolbarTarget =
            (restore !== null && isVisibleDialogControl(restore)
              ? restore
              : null) ??
            popup.querySelector<HTMLElement>("#group-roster-search") ??
            popup.querySelector<HTMLElement>(".group-roster__close");
          toolbarTarget?.focus({ preventScroll: true });
        }
        return;
      }
      if (nextRowIndex < 0) {
        const toolbarTarget =
          popup.querySelector<HTMLElement>("#group-roster-search") ??
          popup.querySelector<HTMLElement>(".group-roster__close");
        toolbarTarget?.focus({ preventScroll: true });
        return;
      }
      const nextRow = rows[nextRowIndex];
      if (nextRow === undefined) return;
      const nextControl =
        (isExclude
          ? nextRow.querySelector<HTMLElement>(".group-roster__exclude")
          : null) ?? nextRow.querySelector<HTMLElement>(".group-roster__pick");
      if (nextControl !== null && isVisibleDialogControl(nextControl)) {
        nextControl.focus({ preventScroll: true });
        revealInScrollContainer(list, nextControl);
      }
      return;
    }
    handleLinearDialogArrows(event, {
      television,
      popup,
      scroller: list,
      horizontalGroups: [".group-roster__toolbar"],
    });
  };

  return (
    <Dialog.Root
      onOpenChange={(open) => {
        if (!open) {
          setQuery("");
          searchEdit.stopEditing();
        }
      }}
    >
      <Dialog.Trigger className="group-roster-trigger">
        <ListFilter aria-hidden="true" />
        {/* An element of its own: a narrow window shows the icon alone. */}
        <span>Choose groups</span>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Backdrop className="group-roster__backdrop" />
        <Dialog.Popup
          ref={popupRef}
          className="group-roster__popup"
          initialFocus={
            television
              ? () => {
                  const target =
                    popupRef.current?.querySelector<HTMLElement>(
                      '.group-roster__row[data-active="true"] .group-roster__pick',
                    ) ??
                    popupRef.current?.querySelector<HTMLElement>(".group-roster__pick") ??
                    popupRef.current?.querySelector<HTMLElement>(".group-roster__close") ??
                    null;
                  if (target !== null) {
                    revealInScrollContainer(listRef.current, target);
                  }
                  return target ?? true;
                }
              : undefined
          }
          onKeyDown={handlePopupKeyDown}
        >
          <header className="group-roster__header">
            <div>
              <Dialog.Title>Channel groups</Dialog.Title>
              <Dialog.Description>
                Pick a group to show in the guide, or hide the ones you never
                watch.
              </Dialog.Description>
            </div>
            <Dialog.Close
              className="group-roster__close"
              aria-label="Close channel groups"
            >
              <X aria-hidden="true" />
            </Dialog.Close>
          </header>

          <div className="group-roster__toolbar">
            <label className="group-roster__search" htmlFor="group-roster-search">
              <Search aria-hidden="true" />
              <input
                id="group-roster-search"
                type="search"
                value={query}
                placeholder="Find a group"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                readOnly={searchEdit.readOnly}
                aria-label="Search channel groups"
                onClick={searchEdit.startEditing}
                onBlur={searchEdit.stopEditing}
                onKeyDown={(event) => {
                  searchEdit.handleKeyDown(event);
                }}
                onChange={(event) => setQuery(event.target.value)}
              />
            </label>
            <p className="group-roster__tally">
              <b>{visibleCount}</b> shown
              {excludedCount > 0 ? (
                <>
                  , <b>{excludedCount}</b> hidden
                </>
              ) : null}
            </p>
            {excludedCount > 0 ? (
              <button
                className="group-roster__restore"
                type="button"
                onClick={() => {
                  const fallback =
                    popupRef.current?.querySelector<HTMLElement>(
                      '.group-roster__row[data-active="true"] .group-roster__pick',
                    ) ??
                    popupRef.current?.querySelector<HTMLElement>(".group-roster__pick");
                  if (fallback != null) {
                    fallback.focus({ preventScroll: true });
                    revealInScrollContainer(listRef.current, fallback);
                  }
                  onRestoreAll();
                }}
              >
                Show all
              </button>
            ) : null}
          </div>

          <div className="group-roster__list" ref={listRef} role="list">
            <RosterAllRow
              active={activeGroup === null}
              onSelect={() => onSelectGroup(null)}
              onPrefetch={() => onPrefetchGroup(null)}
            />
            {matches.map((group) => {
              const excludedFromBoard = excluded.has(group.name);
              return (
                <RosterGroupRow
                  key={group.name === "" ? "ungrouped" : group.name}
                  group={group}
                  active={activeGroup === group.name}
                  excluded={excludedFromBoard}
                  onSelect={() => {
                    if (excludedFromBoard) {
                      onSetExcluded(group.name, false);
                    }
                    onSelectGroup(group.name);
                  }}
                  onPrefetch={() => onPrefetchGroup(group.name)}
                  onSetExcluded={onSetExcluded}
                />
              );
            })}
            {query.trim().length > 0 && matches.length === 0 ? (
              <p className="group-roster__empty" role="status">
                No group matches that search.
              </p>
            ) : null}
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function RosterAllRow({
  active,
  onSelect,
  onPrefetch,
}: {
  readonly active: boolean;
  readonly onSelect: () => void;
  readonly onPrefetch: () => void;
}) {
  return (
    <div className="group-roster__row" data-active={active} role="listitem">
      <Dialog.Close
        className="group-roster__pick"
        type="button"
        aria-label="All channel groups"
        onMouseEnter={onPrefetch}
        onFocus={onPrefetch}
        onClick={onSelect}
      >
        <strong>All channels</strong>
      </Dialog.Close>
    </div>
  );
}

function RosterGroupRow({
  group,
  active,
  excluded,
  onSelect,
  onPrefetch,
  onSetExcluded,
}: {
  readonly group: ChannelGroup;
  readonly active: boolean;
  readonly excluded: boolean;
  readonly onSelect: () => void;
  readonly onPrefetch: () => void;
  readonly onSetExcluded: (name: string, exclude: boolean) => void;
}) {
  const label = groupDisplayName(group.name);
  return (
    <div
      className="group-roster__row"
      data-active={active}
      data-excluded={excluded}
      role="listitem"
    >
      <Dialog.Close
        className="group-roster__pick"
        type="button"
        aria-label={`${label}, ${group.channelCount} channels`}
        onMouseEnter={onPrefetch}
        onFocus={onPrefetch}
        onClick={onSelect}
      >
        <strong>{label}</strong>
        <em>{group.channelCount}</em>
      </Dialog.Close>
      <button
        className="group-roster__exclude"
        type="button"
        aria-pressed={excluded}
        aria-label={excluded ? `Show ${label}` : `Hide ${label}`}
        onClick={() => onSetExcluded(group.name, !excluded)}
      >
        {excluded ? <Eye aria-hidden="true" /> : <EyeOff aria-hidden="true" />}
        <span className="group-roster__exclude-label">
          {excluded ? "Show" : "Hide"}
        </span>
      </button>
    </div>
  );
}

function groupMatches(group: ChannelGroup, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  if (needle.length === 0) {
    return true;
  }
  return groupDisplayName(group.name).toLocaleLowerCase().includes(needle);
}
