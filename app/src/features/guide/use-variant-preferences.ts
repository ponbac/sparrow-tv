import { useCallback, useState } from "react";
import { z } from "zod";
import type { ChannelSummary } from "../../client/contracts";
import { familyKey, type VariantPreferences } from "./guide-families";

const STORAGE_KEY = "sparrow.variant-preferences:v1";
const MAX_STORED_PREFERENCES = 10_000;

const persistedPreferencesSchema = z.strictObject({
  preferences: z
    .array(
      z.strictObject({
        family: z.string().max(4096),
        quality: z.enum(["sd", "hd", "fhd", "uhd"]),
      }),
    )
    .max(MAX_STORED_PREFERENCES),
});

/** Versioned localStorage key for the picture quality chosen per guide row. */
export const VARIANT_PREFERENCES_STORAGE_KEY = STORAGE_KEY;

/** Quality Variant choices persisted in this browser profile. */
export interface VariantPreferenceStore {
  readonly preferences: VariantPreferences;
  /**
   * Records a Channel's picture quality as the choice for its guide row. A
   * Channel that is not a Quality Variant is ignored.
   */
  readonly prefer: (channel: ChannelSummary) => void;
}

/**
 * Owns the picture quality chosen per guide row. Values are parsed from
 * localStorage on first render and written back after each change; missing,
 * unreadable, or invalid storage reads as no choices and never throws.
 */
export function useVariantPreferences(): VariantPreferenceStore {
  const [preferences, setPreferences] = useState<VariantPreferences>(() =>
    typeof localStorage === "undefined"
      ? new Map()
      : readStoredPreferences(localStorage),
  );

  const prefer = useCallback((channel: ChannelSummary) => {
    const variant = channel.variant;
    if (variant === null) {
      return;
    }
    const key = familyKey({ group: channel.group, title: variant.baseName });
    setPreferences((current) => {
      if (current.get(key) === variant.quality) {
        return current;
      }
      const next = new Map(current);
      next.set(key, variant.quality);
      if (typeof localStorage !== "undefined") {
        writeStoredPreferences(localStorage, next);
      }
      return next;
    });
  }, []);

  return { preferences, prefer };
}

function readStoredPreferences(storage: Storage): VariantPreferences {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (raw === null) {
      return new Map();
    }
    const stored: unknown = JSON.parse(raw);
    const parsed = persistedPreferencesSchema.safeParse(stored);
    if (!parsed.success) {
      return new Map();
    }
    return new Map(
      parsed.data.preferences.map(({ family, quality }) => [family, quality]),
    );
  } catch {
    return new Map();
  }
}

/** Quota, private-mode, and disabled-storage failures leave the guide usable. */
function writeStoredPreferences(
  storage: Storage,
  preferences: VariantPreferences,
): void {
  try {
    storage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        preferences: Array.from(preferences, ([family, quality]) => ({
          family,
          quality,
        })),
      }),
    );
  } catch {
    return;
  }
}
