import { Dialog } from "@base-ui/react/dialog";
import { ListFilter, Search, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { ChannelGroup } from "../../client/contracts";
import { groupDisplayName } from "./board-group-roster";
import "./group-roster-dialog.css";

/** Inputs for the Channel Group directory and exclusion cabinet. */
export interface GroupRosterDialogProps {
  readonly groups: readonly ChannelGroup[];
  readonly activeGroup: string | null;
  readonly excluded: ReadonlySet<string>;
  readonly onSelectGroup: (group: string | null) => void;
  readonly onPrefetchGroup: (group: string | null) => void;
  readonly onSetExcluded: (name: string, exclude: boolean) => void;
  readonly onRestoreAll: () => void;
}

/**
 * Opens a searchable Channel Group roster over the guide pane so operators can
 * jump to a group or exclude it from the board.
 */
export function GroupRosterDialog({
  groups,
  activeGroup,
  excluded,
  onSelectGroup,
  onPrefetchGroup,
  onSetExcluded,
  onRestoreAll,
}: GroupRosterDialogProps) {
  const [query, setQuery] = useState("");
  const matches = useMemo(
    () => groups.filter((group) => groupMatches(group, query)),
    [groups, query],
  );
  const excludedCount = excluded.size;
  const visibleCount = Math.max(0, groups.length - excludedCount);

  return (
    <Dialog.Root
      onOpenChange={(open) => {
        if (!open) {
          setQuery("");
        }
      }}
    >
      <Dialog.Trigger
        className="group-roster-trigger"
        aria-label="Channel Group roster"
      >
        <ListFilter aria-hidden="true" />
        Roster
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Backdrop className="group-roster__backdrop" />
        <Dialog.Popup className="group-roster__popup">
          <header className="group-roster__header">
            <div>
              <p>Board roster</p>
              <Dialog.Title>Channel Groups</Dialog.Title>
              <Dialog.Description>
                Search the catalog groups, patch one onto the guide, or exclude
                the dumps you never want on this desk.
              </Dialog.Description>
            </div>
            <Dialog.Close
              className="group-roster__close"
              aria-label="Close Channel Group roster"
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
                aria-label="Search Channel Groups"
                onChange={(event) => setQuery(event.target.value)}
              />
            </label>
            <p className="group-roster__tally">
              <b>{visibleCount}</b> on the board
              {excludedCount > 0 ? (
                <>
                  <i aria-hidden="true" />
                  <b>{excludedCount}</b> excluded
                </>
              ) : null}
            </p>
            {excludedCount > 0 ? (
              <button
                className="group-roster__restore"
                type="button"
                onClick={onRestoreAll}
              >
                Restore all
              </button>
            ) : null}
          </div>

          <div className="group-roster__list" role="list">
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
                No Channel Group matches that search.
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
        aria-label="All Channel Groups"
        onMouseEnter={onPrefetch}
        onFocus={onPrefetch}
        onClick={onSelect}
      >
        <span>All</span>
        <strong>Every group still on the board</strong>
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
        <span>{excluded ? "Excluded" : "Group"}</span>
        <strong>{label}</strong>
        <em>{group.channelCount}</em>
      </Dialog.Close>
      <button
        className="group-roster__exclude"
        type="button"
        aria-pressed={excluded}
        aria-label={
          excluded ? `Restore ${label} to the board` : `Exclude ${label}`
        }
        onClick={() => onSetExcluded(group.name, !excluded)}
      >
        {excluded ? "Restore" : "Exclude"}
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
