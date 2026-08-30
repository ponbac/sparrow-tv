import { describe, expect, it } from "vitest";
import {
  ClientQueryError,
  clientErrorFromQuery,
  successfulQueryResult,
} from "./query-result";

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
});
