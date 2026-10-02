import {
  clientSchemas,
  type ChannelSummary,
  type ChannelVariant,
} from "../client/contracts";

const numbersById = new Map<string, number>();

/**
 * Builds one parsed Channel for a test.
 *
 * The guide folds adjacent rows that share a Channel Number, so every distinct
 * id gets its own number, counted in the order a test file first names it and
 * repeated for the same id. Pass `number` and `variant` only to build the
 * Quality Variants of one guide row.
 */
export function channelFixture(input: {
  readonly id: string;
  readonly name: string;
  readonly group: string;
  readonly number?: number;
  readonly variant?: ChannelVariant | null;
}): ChannelSummary {
  return clientSchemas.channel.parse({
    id: input.id,
    name: input.name,
    group: input.group,
    number: input.number ?? numberFor(input.id),
    variant: input.variant ?? null,
  });
}

function numberFor(id: string): number {
  const known = numbersById.get(id);
  if (known !== undefined) {
    return known;
  }
  const next = numbersById.size + 1;
  numbersById.set(id, next);
  return next;
}
