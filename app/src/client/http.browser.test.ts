import { describe, expect, it } from "vitest";
import { createHttpSparrowClient } from "./http";

describe("browser HTTP Sparrow client", () => {
  it("uses the current window fetch implementation by default", async () => {
    const requests: string[] = [];
    const fetchImplementation: typeof fetch = async (input) => {
      requests.push(requestUrl(input));
      return new Response(
        JSON.stringify({
          sourceConfiguration: "deployment-readonly",
          playbackTransport: "same-origin-http",
          audioTrackSelection: false,
          mpvFailover: false,
        }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      );
    };
    Object.defineProperty(window, "fetch", {
      configurable: true,
      value: fetchImplementation,
    });

    const result = await createHttpSparrowClient().capabilities();

    expect(result.ok).toBe(true);
    expect(requests).toEqual(["/api/v1/capabilities"]);
  });

  it("opens the fixed event route with same-origin credentials", () => {
    const constructions: {
      readonly endpoint: string;
      readonly options: EventSourceInit | undefined;
    }[] = [];
    class BrowserEventSource {
      constructor(endpoint: string, options?: EventSourceInit) {
        constructions.push({ endpoint, options });
      }

      addEventListener(): void {}
      removeEventListener(): void {}
      close(): void {}
    }
    Object.defineProperty(window, "EventSource", {
      configurable: true,
      value: BrowserEventSource,
    });

    const release = createHttpSparrowClient().subscribe(() => undefined);

    expect(constructions).toEqual([
      {
        endpoint: "/api/v1/events",
        options: { withCredentials: true },
      },
    ]);
    release();
  });
});

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") {
    return input;
  }
  if (input instanceof URL) {
    return input.toString();
  }
  return input.url;
}
