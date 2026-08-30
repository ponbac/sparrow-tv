import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import {
  EMPTY_PROGRAMMES,
  FRESH_STATUS,
  MORNING_NEWS,
  MORNING_PROGRAMME_PAGE,
  NO_GUIDE_STATUS,
  WORLD_CHANNEL,
  WORLD_CHANNEL_PAGE,
  FakeSparrowClient,
  continuingSchedulePage,
  failure,
  programmeFixture,
  programmePage,
  renderConsole,
  searchResults,
  submitSearch,
  success,
} from "./search-console.test-support";

afterEach(cleanup);

describe("SearchConsole selected schedule", () => {
  it("keeps Channel search complete when no EPG Source is configured", async () => {
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, EMPTY_PROGRAMMES)),
        ),
    });
    const user = userEvent.setup();
    renderConsole(client, NO_GUIDE_STATUS);

    expect(screen.getByText("GUIDE ABSENT")).toBeVisible();
    await submitSearch(user, "news");

    expect(await screen.findByText("World News")).toBeVisible();
    expect(
      screen.getByText(/Programme search is unavailable because no EPG Source/),
    ).toBeVisible();

    await user.click(screen.getByRole("button", { name: /World News/ }));
    expect(
      await screen.findByText(/Channel remains available without a schedule/),
    ).toBeVisible();
    expect(client.scheduleInputs).toHaveLength(1);
    expect(client.scheduleInputs[0]?.id).toBe(WORLD_CHANNEL.id);
  });

  it("opens a Programme result's owning Channel schedule in a Guide-enriched catalog", async () => {
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, MORNING_PROGRAMME_PAGE)),
        ),
      schedule: () => Promise.resolve(success(MORNING_PROGRAMME_PAGE)),
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);

    await submitSearch(user, "morning");
    await user.click(
      await screen.findByRole("button", {
        name: /Morning News.*Open Channel schedule/,
      }),
    );

    const schedule = screen.getByRole("complementary", {
      name: "Programme schedule",
    });
    expect(await within(schedule).findByText("World News")).toBeVisible();
    expect(within(schedule).getByText("Morning News")).toBeVisible();
    expect(within(schedule).getByText("A safe fixture rundown.")).toBeVisible();
    expect(schedule.querySelector("time")).toHaveAttribute(
      "datetime",
      MORNING_NEWS.startsAt,
    );
    expect(client.scheduleInputs[0]?.id).toBe(MORNING_NEWS.channelId);
  });

  it("paginates a selected Channel schedule through contract-valid pages", async () => {
    const firstSchedule = continuingSchedulePage(MORNING_NEWS, "schedule-next");
    const evening = programmePage(
      programmeFixture(
        "Evening News",
        "2026-08-30T16:00:00.000Z",
        "2026-08-30T17:00:00.000Z",
      ),
      7,
    );
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, EMPTY_PROGRAMMES)),
        ),
      schedule: (input) =>
        Promise.resolve(
          success(input.cursor === firstSchedule.next ? evening : firstSchedule),
        ),
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "world");
    await user.click(await screen.findByRole("button", { name: /World News/ }));

    const schedule = await screen.findByRole("list", {
      name: "Programme times for World News",
    });
    expect(schedule).toHaveAttribute("tabindex", "0");
    await user.click(
      screen.getByRole("button", { name: "Later Programmes +" }),
    );
    expect(await within(schedule).findByText("Evening News")).toBeVisible();
    expect(client.scheduleInputs[1]?.cursor).toBe(firstSchedule.next);
    expect(client.scheduleInputs[1]?.afterStartsAt).toBe(
      firstSchedule.items.at(-1)?.startsAt,
    );
    expect(client.scheduleInputs[1]?.previousCursors).toEqual([]);
  });

  it("retains a schedule after stale-cursor failure and restarts at the current generation", async () => {
    const firstSchedule = continuingSchedulePage(MORNING_NEWS, "schedule-next");
    const currentSchedule = programmePage(
      programmeFixture(
        "Current Rundown",
        MORNING_NEWS.startsAt,
        MORNING_NEWS.endsAt,
        MORNING_NEWS.description,
      ),
      8,
    );
    let cursorRejected = false;
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, EMPTY_PROGRAMMES)),
        ),
      schedule: (input) => {
        if (input.cursor === firstSchedule.next) {
          cursorRejected = true;
          return Promise.resolve(
            failure({
              _tag: "stale-cursor",
              current: currentSchedule.generation,
            }),
          );
        }
        return Promise.resolve(
          success(cursorRejected ? currentSchedule : firstSchedule),
        );
      },
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "world");
    await user.click(await screen.findByRole("button", { name: /World News/ }));
    await user.click(
      await screen.findByRole("button", { name: "Later Programmes +" }),
    );

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/Earlier results remain visible/)).toBeVisible();
    expect(screen.getByText("Morning News")).toBeVisible();
    await user.click(
      within(alert).getByRole("button", {
        name: "Restart on current catalog",
      }),
    );
    expect(await screen.findByText("Current Rundown")).toBeVisible();
    expect(screen.queryByText("Morning News")).not.toBeInTheDocument();
  });

  it("rejects a successful cross-generation schedule page before rendering it", async () => {
    const firstSchedule = continuingSchedulePage(MORNING_NEWS, "schedule-next");
    const newerSchedule = programmePage(
      programmeFixture(
        "New Generation Programme",
        "2026-08-30T16:00:00.000Z",
        "2026-08-30T17:00:00.000Z",
        MORNING_NEWS.description,
      ),
      8,
    );
    let generationCrossed = false;
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, EMPTY_PROGRAMMES)),
        ),
      schedule: (input) => {
        if (input.cursor === firstSchedule.next) {
          generationCrossed = true;
          return Promise.resolve(success(newerSchedule));
        }
        return Promise.resolve(
          success(generationCrossed ? newerSchedule : firstSchedule),
        );
      },
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "world");
    await user.click(await screen.findByRole("button", { name: /World News/ }));
    await user.click(
      await screen.findByRole("button", { name: "Later Programmes +" }),
    );

    expect(
      await screen.findByText(/catalog changed during pagination/),
    ).toBeVisible();
    expect(screen.getByText("Morning News")).toBeVisible();
    expect(screen.queryByText("New Generation Programme")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Restart scan" }));
    expect(await screen.findByText("New Generation Programme")).toBeVisible();
  });

  it("never implies a guessed Guide association for an empty matched schedule", async () => {
    const client = new FakeSparrowClient({
      search: () =>
        Promise.resolve(
          success(searchResults(WORLD_CHANNEL_PAGE, EMPTY_PROGRAMMES)),
        ),
    });
    const user = userEvent.setup();
    renderConsole(client, FRESH_STATUS);
    await submitSearch(user, "world");
    await user.click(await screen.findByRole("button", { name: /World News/ }));

    expect(await screen.findByText(/Sparrow never guesses/)).toBeVisible();
    expect(screen.getByText(/Unmatched records stay unassociated/)).toBeVisible();
  });
});
