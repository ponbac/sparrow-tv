import { useCallback, useState } from "react";
import {
  readStoredExclusions,
  setGroupExcluded,
  writeStoredExclusions,
} from "./board-group-roster";

/** Board-local Channel Group exclusions persisted in this browser profile. */
export interface BoardGroupExclusions {
  readonly excluded: ReadonlySet<string>;
  readonly setExcluded: (name: string, exclude: boolean) => void;
  readonly restoreAll: () => void;
}

/**
 * Owns excluded Channel Group names for the Programme guide. Values are parsed
 * from localStorage on first render and written back after each change.
 */
export function useBoardGroupExclusions(): BoardGroupExclusions {
  const [excluded, setExcludedState] = useState<ReadonlySet<string>>(() =>
    typeof localStorage === "undefined"
      ? new Set()
      : readStoredExclusions(localStorage),
  );

  const persist = (next: ReadonlySet<string>) => {
    if (typeof localStorage !== "undefined") {
      writeStoredExclusions(localStorage, next);
    }
  };

  const setExcluded = useCallback((name: string, exclude: boolean) => {
    setExcludedState((current) => {
      const next = setGroupExcluded(current, name, exclude);
      if (next === current) {
        return current;
      }
      persist(next);
      return next;
    });
  }, []);

  const restoreAll = useCallback(() => {
    setExcludedState((current) => {
      if (current.size === 0) {
        return current;
      }
      const next = new Set<string>();
      persist(next);
      return next;
    });
  }, []);

  return { excluded, setExcluded, restoreAll };
}
