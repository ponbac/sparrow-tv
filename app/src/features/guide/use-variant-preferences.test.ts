import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { channelFixture } from "../../test/channel-fixture";
import { familyKey } from "./guide-families";
import {
  useVariantPreferences,
  VARIANT_PREFERENCES_STORAGE_KEY,
} from "./use-variant-preferences";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const SVT1_KEY = familyKey({ group: "Sweden", title: "SVT1" });

const SVT1_HD = channelFixture({
  id: "svt1-hd",
  name: "SVT1 HD",
  group: "Sweden",
  variant: { quality: "hd", baseName: "SVT1" },
});

describe("useVariantPreferences", () => {
  it("keeps a chosen quality for the next visit", () => {
    const first = renderHook(() => useVariantPreferences());
    expect(first.result.current.preferences.size).toBe(0);

    act(() => first.result.current.prefer(SVT1_HD));

    expect(first.result.current.preferences.get(SVT1_KEY)).toBe("hd");
    first.unmount();
    const next = renderHook(() => useVariantPreferences());
    expect(Array.from(next.result.current.preferences)).toEqual([
      [SVT1_KEY, "hd"],
    ]);
  });

  it("records nothing for a Channel that is not a Quality Variant", () => {
    const { result } = renderHook(() => useVariantPreferences());

    act(() =>
      result.current.prefer(
        channelFixture({ id: "world-news", name: "World News", group: "News" }),
      ),
    );

    expect(result.current.preferences.size).toBe(0);
    expect(localStorage.getItem(VARIANT_PREFERENCES_STORAGE_KEY)).toBeNull();
  });

  it.each([
    ["text that is not JSON", "{"],
    ["another shape", JSON.stringify({ preferences: { [SVT1_KEY]: "hd" } })],
    [
      "an unknown quality",
      JSON.stringify({ preferences: [{ family: SVT1_KEY, quality: "8k" }] }),
    ],
  ])("starts without choices when storage holds %s", (_, stored) => {
    localStorage.setItem(VARIANT_PREFERENCES_STORAGE_KEY, stored);

    const { result } = renderHook(() => useVariantPreferences());

    expect(result.current.preferences.size).toBe(0);
  });
});
