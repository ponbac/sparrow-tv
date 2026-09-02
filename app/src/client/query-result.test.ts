import { describe, expect, it } from "vitest";
import {
  ClientQueryError,
  clientErrorFromQuery,
  generationBoundResult,
  successfulQueryResult,
} from "./query-result";
import type { CatalogGeneration } from "./contracts";

describe("React Query client result boundary", () => {
  it("returns successful client data unchanged", async () => {
    const result = { ok: true, value: "usable" } as const;

    await expect(successfulQueryResult(Promise.resolve(result))).resolves.toBe(
      result,
    );
  });

  it("moves a closed client failure into the query error channel", async () => {
    const error = { _tag: "authentication-required" } as const;
    const result = {
      ok: false,
      error,
    } as const;

    const failure = await successfulQueryResult(Promise.resolve(result)).catch(
      (error: unknown) => error,
    );
    expect(failure).toBeInstanceOf(ClientQueryError);
    expect(clientErrorFromQuery(failure)).toEqual(error);
  });

  it("minimizes unexpected thrown values without exposing their message", () => {
    const privateCanary = "https://user:secret@provider.invalid/private.m3u";
    const recovered = clientErrorFromQuery(new Error(privateCanary));

    expect(recovered).toEqual({
      _tag: "transport",
      retryable: true,
      message: "The hosted desk did not complete this catalog request.",
    });
    expect(JSON.stringify(recovered)).not.toContain(privateCanary);
  });

  it("rejects a page from outside the expected catalog generation", async () => {
    const operation = Promise.resolve({
      ok: true,
      value: {
        generation: 8 as CatalogGeneration,
        items: ["replacement"],
        next: null,
      },
    } as const);

    const failure = await generationBoundResult(
      operation,
      7 as CatalogGeneration,
    ).catch((error: unknown) => error);

    expect(clientErrorFromQuery(failure)).toEqual({
      _tag: "stale-cursor",
      current: 8,
    });
  });
});
