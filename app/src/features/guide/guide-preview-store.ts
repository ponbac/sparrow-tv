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
  readonly set: (focus: PreviewFocus, afterPaint?: boolean) => void;
  readonly cancelPending: () => void;
  readonly subscribe: (listener: () => void) => () => void;
}

/** Creates the store one guide's rows write their focus to. */
export function createPreviewStore(): PreviewStore {
  let current: PreviewFocus | null = null;
  const listeners = new Set<() => void>();
  let frame: number | null = null;
  let timer: number | null = null;
  let pending: PreviewFocus | null = null;
  const publish = (focus: PreviewFocus) => {
    if (current?.number === focus.number && current.programme === focus.programme) return;
    current = focus;
    listeners.forEach((listener) => listener());
  };
  const cancelPending = () => {
    if (frame !== null) window.cancelAnimationFrame(frame);
    if (timer !== null) window.clearTimeout(timer);
    frame = timer = null;
    pending = null;
  };
  return {
    get: () => current,
    cancelPending,
    set: (focus, afterPaint = false) => {
      if (!afterPaint || current === null) {
        cancelPending();
        publish(focus);
        return;
      }
      // Let the browser draw the newly focused Channel first. Repeated
      // arrows in the same frame publish only the most recent preview.
      pending = focus;
      if (frame !== null || timer !== null) return;
      frame = window.requestAnimationFrame(() => {
        frame = null;
        timer = window.setTimeout(() => {
          timer = null;
          if (pending !== null) publish(pending);
          pending = null;
        }, 0);
      });
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
