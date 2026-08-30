import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clientSchemas, type CatalogStatus } from "../../client/contracts";
import { SourceStatusDesk } from "./source-status-desk";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const FRESH_STATUS = clientSchemas.status.parse({
  generation: 7,
  configuration: { configured: true, epgConfigured: true },
  m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:00:00Z" },
  epg: { _tag: "fresh", validatedAt: "2026-08-30T10:00:01Z" },
});

const INDEPENDENT_GUIDE_FAILURE = clientSchemas.status.parse({
  ...FRESH_STATUS,
  epg: {
    _tag: "failed",
    validatedAt: "2026-08-30T09:00:00Z",
    failure: {
      _tag: "invalid-epg-format",
      source: "epg",
      reason: "malformed-xml",
    },
    nextAttemptAt: "2026-08-30T10:05:00Z",
  },
});

describe("SourceStatusDesk", () => {
  it("presents Channel freshness independently from a retained Guide failure", () => {
    renderDesk(INDEPENDENT_GUIDE_FAILURE);

    expect(screen.getByText("FRESH")).toBeVisible();
    expect(screen.getByText("FAILED / RETAINED")).toBeVisible();
    expect(screen.getByText("failure / epg / malformed-xml")).toBeVisible();
    expect(screen.getByRole("button", { name: "Refresh sources" })).toBeEnabled();
  });

  it("keeps each source card on its own progress while a manual request is pending", () => {
    const independentlyRefreshing = clientSchemas.status.parse({
      ...FRESH_STATUS,
      epg: {
        _tag: "refreshing",
        validatedAt: "2026-08-30T10:00:01Z",
        startedAt: "2026-08-30T10:02:00Z",
      },
    });

    render(
      <SourceStatusDesk
        status={independentlyRefreshing}
        refreshing={true}
        refreshResult={null}
        latestEvent={null}
        onRefresh={() => undefined}
      />,
    );

    expect(screen.getByText("FRESH")).toBeVisible();
    expect(screen.getByText("REFRESHING / RETAINED")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Refresh in progress" }),
    ).toBeDisabled();
  });

  it("distinguishes an unavailable Channel source from an absent Guide", () => {
    const unavailable = clientSchemas.status.parse({
      generation: null,
      configuration: { configured: true, epgConfigured: false },
      m3u: {
        _tag: "unavailable",
        failure: {
          _tag: "source-access",
          source: "m3u",
          reason: "timed-out",
          retryAfterSeconds: 45,
        },
      },
      epg: null,
    });

    renderDesk(unavailable);

    expect(screen.getByText("UNAVAILABLE")).toBeVisible();
    expect(screen.getByText("NOT CONFIGURED")).toBeVisible();
    expect(
      screen.getByText("failure / m3u / timed-out / retry 45s"),
    ).toBeVisible();
    expect(
      screen.getByText(
        /Browse, search, and playback require a validated Channel snapshot/,
      ),
    ).toBeVisible();
  });

  it("keeps Channel features available when only the Guide is absent", () => {
    const withoutGuide = clientSchemas.status.parse({
      ...FRESH_STATUS,
      configuration: { configured: true, epgConfigured: false },
      epg: null,
    });

    renderDesk(withoutGuide);

    expect(
      screen.getByText(/Channel browse, search, and playback remain available/),
    ).toBeVisible();
  });

  it("reports each manual outcome while a Guide failure leaves retained data usable", () => {
    const result = clientSchemas.refreshReport.parse({
      trigger: "manual",
      m3u: {
        _tag: "updated",
        validatedAt: "2026-08-30T10:02:00Z",
      },
      epg: {
        _tag: "failed",
        failure: {
          _tag: "snapshot",
          source: "epg",
          operation: "activate",
          reason: "corrupt",
        },
        nextAttemptAt: "2026-08-30T10:05:00Z",
      },
      status: INDEPENDENT_GUIDE_FAILURE,
    });
    render(
      <SourceStatusDesk
        status={INDEPENDENT_GUIDE_FAILURE}
        refreshing={false}
        refreshResult={{ ok: true, value: result }}
        latestEvent={null}
        onRefresh={() => undefined}
      />,
    );

    const feedback = screen.getByRole("alert");
    expect(within(feedback).getByText("Guide source refresh failed")).toBeVisible();
    expect(
      within(feedback).getByText(/Channel source: updated/),
    ).toBeVisible();
    expect(
      within(feedback).getByText(/Guide source: failed \/ epg \/ activate \/ corrupt/),
    ).toBeVisible();
    expect(within(feedback).getByText(/Browsing and playback stay available/)).toBeVisible();
  });

  it("does not promise playback after an installed Guide-only refresh failure", () => {
    const result = clientSchemas.refreshReport.parse({
      trigger: "manual",
      m3u: {
        _tag: "updated",
        validatedAt: "2026-08-30T10:02:00Z",
      },
      epg: {
        _tag: "failed",
        failure: {
          _tag: "snapshot",
          source: "epg",
          operation: "activate",
          reason: "corrupt",
        },
        nextAttemptAt: "2026-08-30T10:05:00Z",
      },
      status: INDEPENDENT_GUIDE_FAILURE,
    });

    render(
      <SourceStatusDesk
        status={INDEPENDENT_GUIDE_FAILURE}
        refreshing={false}
        refreshResult={{ ok: true, value: result }}
        latestEvent={null}
        onRefresh={() => undefined}
        playbackAvailable={false}
      />,
    );

    const feedback = screen.getByRole("alert");
    expect(
      within(feedback).getByText(/Channel browsing and search stay available/),
    ).toBeVisible();
    expect(within(feedback).queryByText(/playback/i)).not.toBeInTheDocument();
  });

  it.each([
    ["Channel-only", false],
    ["combined Channel and Guide", true],
  ] as const)(
    "ties %s refresh failure availability to the retained Channel snapshot",
    (_, guideFailed) => {
      const retainedFailureStatus = clientSchemas.status.parse({
        ...FRESH_STATUS,
        m3u: {
          _tag: "failed",
          validatedAt: "2026-08-30T10:00:00Z",
          failure: {
            _tag: "source-access",
            source: "m3u",
            reason: "timed-out",
            retryAfterSeconds: 45,
          },
          nextAttemptAt: "2026-08-30T10:05:00Z",
        },
        epg: guideFailed ? INDEPENDENT_GUIDE_FAILURE.epg : FRESH_STATUS.epg,
      });
      const result = clientSchemas.refreshReport.parse({
        trigger: "manual",
        m3u: {
          _tag: "failed",
          failure: {
            _tag: "source-access",
            source: "m3u",
            reason: "timed-out",
            retryAfterSeconds: 45,
          },
          nextAttemptAt: "2026-08-30T10:05:00Z",
        },
        epg: guideFailed
          ? {
              _tag: "failed",
              failure: {
                _tag: "snapshot",
                source: "epg",
                operation: "activate",
                reason: "corrupt",
              },
              nextAttemptAt: "2026-08-30T10:05:00Z",
            }
          : {
              _tag: "updated",
              validatedAt: "2026-08-30T10:02:01Z",
            },
        status: retainedFailureStatus,
      });

      render(
        <SourceStatusDesk
          status={retainedFailureStatus}
          refreshing={false}
          refreshResult={{ ok: true, value: result }}
          latestEvent={null}
          onRefresh={() => undefined}
          playbackAvailable={false}
        />,
      );

      const feedback = screen.getByRole("alert");
      expect(
        within(feedback).getByText(
          guideFailed
            ? "Channel source and Guide source refresh failed"
            : "Channel source refresh failed",
        ),
      ).toBeVisible();
      expect(
        within(feedback).getByText(
          /The Channel source failed, but its last validated snapshot remains in service/,
        ),
      ).toBeVisible();
      expect(
        within(feedback).queryByText(/because the Channel source completed independently/),
      ).not.toBeInTheDocument();
      expect(within(feedback).queryByText(/playback/i)).not.toBeInTheDocument();
    },
  );

  it("reports a fully successful manual refresh with both source outcomes", () => {
    const refreshedStatus = clientSchemas.status.parse({
      ...FRESH_STATUS,
      generation: 8,
      m3u: { _tag: "fresh", validatedAt: "2026-08-30T10:02:00Z" },
      epg: { _tag: "fresh", validatedAt: "2026-08-30T10:02:01Z" },
    });
    const result = clientSchemas.refreshReport.parse({
      trigger: "manual",
      m3u: {
        _tag: "updated",
        validatedAt: "2026-08-30T10:02:00Z",
      },
      epg: {
        _tag: "not-modified",
        validatedAt: "2026-08-30T10:02:01Z",
      },
      status: refreshedStatus,
    });

    render(
      <SourceStatusDesk
        status={refreshedStatus}
        refreshing={false}
        refreshResult={{ ok: true, value: result }}
        latestEvent={null}
        onRefresh={() => undefined}
      />,
    );

    const feedback = screen.getByRole("status");
    expect(within(feedback).getByText("Manual refresh complete")).toBeVisible();
    expect(within(feedback).getByText(/Channel source: updated/)).toBeVisible();
    expect(
      within(feedback).getByText(/Guide source: validated \/ unchanged/),
    ).toBeVisible();
    expect(within(feedback).getByText(/Catalog generation 8/)).toBeVisible();
  });

  it("treats a missing transport response as ambiguous and points to reconciled status", () => {
    render(
      <SourceStatusDesk
        status={FRESH_STATUS}
        refreshing={false}
        refreshResult={{
          ok: false,
          error: {
            _tag: "transport",
            retryable: true,
            message: "A safe transport failure.",
          },
        }}
        latestEvent={null}
        onRefresh={() => undefined}
      />,
    );

    const feedback = screen.getByRole("alert");
    expect(
      within(feedback).getByText("Refresh result was not received"),
    ).toBeVisible();
    expect(within(feedback).getByText(/may still have completed/)).toBeVisible();
    expect(within(feedback).queryByText(/catalog is unchanged/)).not.toBeInTheDocument();
  });

  it("copies only deterministic safe fields and resets copied state for a new snapshot", async () => {
    const user = userEvent.setup();
    const writes: string[] = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (value: string) => {
          writes.push(value);
          return Promise.resolve();
        },
      },
    });
    const privateCanary = "https://user:secret@provider.invalid/private.m3u";
    const { rerender } = render(
      <SourceStatusDesk
        status={INDEPENDENT_GUIDE_FAILURE}
        refreshing={false}
        refreshResult={{
          ok: false,
          error: {
            _tag: "transport",
            retryable: true,
            message: privateCanary,
          },
        }}
        latestEvent={null}
        onRefresh={() => undefined}
      />,
    );
    await user.click(screen.getByText("Safe diagnostics / copyable"));
    expect(
      screen.getByRole("region", { name: "Safe source diagnostics" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeVisible();
    expect(writes).toHaveLength(1);
    expect(writes[0]).toContain("epg.failure-reason=malformed-xml");
    expect(writes[0]).not.toContain(privateCanary);

    const publishedEvent = clientSchemas.sparrowEvent.parse({
      _tag: "catalog-published",
      occurredAt: "2026-08-30T10:03:00Z",
      generation: 8,
    });

    rerender(
      <SourceStatusDesk
        status={FRESH_STATUS}
        refreshing={false}
        refreshResult={null}
        latestEvent={publishedEvent}
        onRefresh={() => undefined}
      />,
    );

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Copy diagnostics" }),
      ).toBeVisible(),
    );
  });
});

function renderDesk(status: CatalogStatus): void {
  render(
    <SourceStatusDesk
      status={status}
      refreshing={false}
      refreshResult={null}
      latestEvent={null}
      onRefresh={() => undefined}
    />,
  );
}
