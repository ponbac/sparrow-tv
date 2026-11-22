/*
 * What the TV Guide's focus rests on, for the band's preview panel. Kept
 * outside React so a D-pad move re-renders the panel alone.
 */

/** The guide row, and in the timeline the Programme, that focus rests on. */
export interface PreviewFocus {
  readonly number: number;
  /**
   * The focused Programme in the timeline, by its key among the row's
   * Programmes (`programmeKeys`); null for a whole row.
   */
  readonly programme: string | null;
}

/** Holds what focus rests on, outside React, so a move re-renders the preview alone. */
export interface PreviewStore {
  readonly get: () => PreviewFocus | null;
  readonly set: (focus: PreviewFocus) => void;
  readonly subscribe: (listener: () => void) => () => void;
}

/** Creates the store one guide's rows write their focus to. */
export function createPreviewStore(): PreviewStore {
  let current: PreviewFocus | null = null;
  const listeners = new Set<() => void>();
  return {
    get: () => current,
    set: (focus) => {
      if (
        current !== null &&
        current.number === focus.number &&
        current.programme === focus.programme
      ) {
        return;
      }
      current = focus;
      listeners.forEach((listener) => listener());
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

/** Reads the guide focus a row last wrote from a focus event's target. */
export function previewFocusOf(target: EventTarget | null): PreviewFocus | null {
  if (!(target instanceof Element)) return null;
  const row = target.closest<HTMLElement>(".programme-guide__row[data-number]");
  const number = Number(row?.dataset.number);
  if (row === null || !Number.isFinite(number)) return null;
  const cell = target.closest<HTMLElement>("[data-programme-key]");
  return { number, programme: cell?.dataset.programmeKey ?? null };
}
