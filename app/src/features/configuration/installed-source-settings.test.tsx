import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clientSchemas,
  type CatalogStatus,
  type ClientResult,
  type SourceConfigurationInput,
} from "../../client/contracts";
import { InstalledSourceSettings } from "./installed-source-settings";

afterEach(() => {
  cleanup();
  localStorage.clear();
  sessionStorage.clear();
  vi.restoreAllMocks();
});

const FRESH_STATUS = clientSchemas.status.parse({
  generation: 11,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

describe("installed source settings", () => {
  it("keeps raw locations out of React Query, storage, history, and logs", async () => {
    const sourceLocation = "https://user:secret@provider.invalid/list.m3u";
    const guideLocation = "https://user:secret@provider.invalid/guide.xml";
    const replacement = deferred<ClientResult<CatalogStatus>>();
    const inputs: SourceConfigurationInput[] = [];
    const queryClient = new QueryClient();
    const onApplied = vi.fn();
    const consoleSpies = [
      vi.spyOn(console, "log").mockImplementation(() => undefined),
      vi.spyOn(console, "warn").mockImplementation(() => undefined),
      vi.spyOn(console, "error").mockImplementation(() => undefined),
    ];
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={queryClient}>
        <InstalledSourceSettings
          client={{
            replaceSourceConfiguration: (input) => {
              inputs.push(input);
              return replacement.promise;
            },
          }}
          status={null}
          onApplied={onApplied}
        />
      </QueryClientProvider>,
    );

    const m3u = screen.getByLabelText("Required / Channel source");
    const epg = screen.getByLabelText("Optional / Guide source");
    expect(m3u).not.toHaveAttribute("name");
    expect(epg).not.toHaveAttribute("name");
    await user.type(m3u, sourceLocation);
    await user.type(epg, guideLocation);
    await user.click(
      screen.getByRole("button", { name: "Build local catalog" }),
    );

    expect(inputs).toHaveLength(1);
    expect(JSON.stringify(queryClient.getQueryCache().getAll())).not.toContain(
      "provider.invalid",
    );
    expect(queryClient.getMutationCache().getAll()).toEqual([]);
    expect(JSON.stringify(localStorage)).not.toContain("provider.invalid");
    expect(JSON.stringify(sessionStorage)).not.toContain("provider.invalid");
    expect(window.location.href).not.toContain("provider.invalid");
    expect(consoleSpies.every((spy) => spy.mock.calls.length === 0)).toBe(true);

    await act(async () => {
      replacement.resolve({ ok: true, value: FRESH_STATUS });
      await replacement.promise;
    });

    expect(m3u).toHaveValue("");
    expect(epg).toHaveValue("");
    expect(onApplied).toHaveBeenCalledWith(FRESH_STATUS);
    expect(screen.getByRole("status")).toHaveTextContent(
      "Configuration saved",
    );
    expect(JSON.stringify(queryClient.getQueryCache().getAll())).not.toContain(
      "provider.invalid",
    );
    expect(queryClient.getMutationCache().getAll()).toEqual([]);
    expect(consoleSpies.every((spy) => spy.mock.calls.length === 0)).toBe(true);
  });

  it("clears DOM fields and aborts the active replacement on unmount", async () => {
    const replacement = deferred<ClientResult<CatalogStatus>>();
    const inputs: SourceConfigurationInput[] = [];
    const user = userEvent.setup();
    const view = render(
      <InstalledSourceSettings
        client={{
          replaceSourceConfiguration: (input) => {
            inputs.push(input);
            return replacement.promise;
          },
        }}
        status={null}
        onApplied={() => undefined}
      />,
    );
    const m3u = screen.getByLabelText<HTMLInputElement>(
      "Required / Channel source",
    );
    const epg = screen.getByLabelText<HTMLInputElement>(
      "Optional / Guide source",
    );
    await user.type(m3u, "https://provider.invalid/private.m3u");
    await user.type(epg, "https://provider.invalid/private.xml");
    await user.click(
      screen.getByRole("button", { name: "Build local catalog" }),
    );

    const submitted = requireFirst(inputs);
    expect(submitted.signal?.aborted).toBe(false);
    view.unmount();

    expect(submitted.signal?.aborted).toBe(true);
    expect(m3u.value).toBe("");
    expect(epg.value).toBe("");
  });

  it("sends an empty optional Guide field as null", async () => {
    const inputs: SourceConfigurationInput[] = [];
    const user = userEvent.setup();
    render(
      <InstalledSourceSettings
        client={{
          replaceSourceConfiguration: (input) => {
            inputs.push(input);
            return Promise.resolve({ ok: true, value: FRESH_STATUS });
          },
        }}
        status={null}
        onApplied={() => undefined}
      />,
    );

    await user.type(
      screen.getByLabelText("Required / Channel source"),
      "https://provider.invalid/list.m3u",
    );
    await user.click(
      screen.getByRole("button", { name: "Build local catalog" }),
    );

    expect(requireFirst(inputs).epgLocation).toBeNull();
  });
});

interface Deferred<Value> {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
}

function deferred<Value>(): Deferred<Value> {
  let resolver: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((resolve) => {
    resolver = resolve;
  });
  return {
    promise,
    resolve: (value) => {
      if (resolver === undefined) {
        throw new Error("deferred resolver was not initialized");
      }
      resolver(value);
    },
  };
}

function requireFirst<Value>(values: readonly Value[]): Value {
  const first = values[0];
  if (first === undefined) {
    throw new Error("expected one submitted input");
  }
  return first;
}
