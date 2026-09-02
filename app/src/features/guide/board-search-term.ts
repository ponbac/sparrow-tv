const textEncoder = new TextEncoder();

/** Maximum UTF-8 size of a catalog search term, matching the core contract. */
export const MAX_SEARCH_TERM_BYTES = 256;

/** Delay before a typed board-search term is sent to the catalog. */
export const SEARCH_DEBOUNCE_MS = 90;

/**
 * Normalizes a search field value into the catalog request term: NFKC, case
 * fold, trim, and collapse interior whitespace. Empty input stays empty.
 */
export function canonicalSearchTerm(value: string): string {
  return value.normalize("NFKC").toLowerCase().trim().replace(/\s+/gu, " ");
}

/**
 * True when both the raw field value and its canonical request term fit the
 * catalog search-term byte budget. Oversized input must not start a query.
 */
export function searchTermFits(value: string): boolean {
  const canonical = canonicalSearchTerm(value);
  return (
    textEncoder.encode(value).byteLength <= MAX_SEARCH_TERM_BYTES &&
    textEncoder.encode(canonical).byteLength <= MAX_SEARCH_TERM_BYTES
  );
}
