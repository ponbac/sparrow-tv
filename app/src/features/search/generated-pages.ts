import type {
  CatalogGeneration,
  ClientError,
  ClientResult,
} from "../../client/contracts";

interface GeneratedValue {
  readonly generation: CatalogGeneration;
}

/** Collects only values that belong to the first accepted immutable generation. */
export function collectGenerationItems<Value extends GeneratedValue, Item>(
  pages: readonly ClientResult<Value>[] | undefined,
  expectedGeneration: CatalogGeneration | null,
  items: (value: Value) => readonly Item[],
): readonly Item[] {
  return (
    pages?.flatMap((page) =>
      page.ok && page.value.generation === expectedGeneration
        ? items(page.value)
        : [],
    ) ?? []
  );
}

/** Returns the first successful immutable generation in a result sequence. */
export function firstGeneration<Value extends GeneratedValue>(
  pages: readonly ClientResult<Value>[] | undefined,
): CatalogGeneration | null {
  return pages?.find((page) => page.ok)?.value.generation ?? null;
}

/** Returns the first expected typed failure while retaining earlier successes. */
export function firstResultError<Value>(
  pages: readonly ClientResult<Value>[] | undefined,
): ClientError | null {
  if (pages === undefined) {
    return null;
  }
  for (const page of pages) {
    if (!page.ok) {
      return page.error;
    }
  }
  return null;
}

/** Detects a successful page that cannot be combined with the accepted generation. */
export function hasUnexpectedGeneration<Value extends GeneratedValue>(
  pages: readonly ClientResult<Value>[] | undefined,
  expectedGeneration: CatalogGeneration | null,
): boolean {
  return (
    pages?.some(
      (page) => page.ok && page.value.generation !== expectedGeneration,
    ) ?? false
  );
}
