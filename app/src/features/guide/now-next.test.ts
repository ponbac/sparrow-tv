import { describe, expect, it } from "vitest";
import { clientSchemas, type ProgrammeSlot } from "../../client/contracts";
import {
  elapsedFraction,
  minutesLeft,
  nextAfter,
  programmeAt,
} from "./now-next";

const QUIZ = programme("Quiz", "20:00", "20:30");
const FILM = programme("Film", "20:30", "21:30");
const NEWS = programme("News", "22:00", "22:30");
const EVENING = [QUIZ, FILM, NEWS];

describe("programmeAt", () => {
  it("finds the Programme airing at an instant, now or later", () => {
    expect(programmeAt(EVENING, at("20:45"))).toBe(FILM);
    expect(programmeAt(EVENING, at("22:10"))).toBe(NEWS);
  });

  it("uses half-open intervals at the exact Programme end", () => {
    expect(programmeAt(EVENING, at("20:00"))).toBe(QUIZ);
    expect(programmeAt(EVENING, at("20:30"))).toBe(FILM);
    expect(programmeAt(EVENING, at("22:30"))).toBeNull();
  });

  it("finds nothing in a gap, before the first Programme, or with none", () => {
    expect(programmeAt(EVENING, at("21:45"))).toBeNull();
    expect(programmeAt(EVENING, at("19:00"))).toBeNull();
    expect(programmeAt([], at("20:45"))).toBeNull();
  });
});

describe("nextAfter", () => {
  it("finds the first Programme starting after an instant", () => {
    expect(nextAfter(EVENING, at("19:00"))).toBe(QUIZ);
    expect(nextAfter(EVENING, at("20:45"))).toBe(NEWS);
    // In a gap the next one is still the one that follows.
    expect(nextAfter(EVENING, at("21:45"))).toBe(NEWS);
  });

  it("does not count a Programme starting at that very instant", () => {
    expect(nextAfter(EVENING, at("20:30"))).toBe(NEWS);
    expect(nextAfter(EVENING, at("20:29"))).toBe(FILM);
  });

  it("finds nothing after the last start", () => {
    expect(nextAfter(EVENING, at("22:00"))).toBeNull();
    expect(nextAfter([], at("20:00"))).toBeNull();
  });
});

describe("minutesLeft", () => {
  it("rounds the remaining time up to whole minutes", () => {
    expect(minutesLeft(FILM, at("20:45"))).toBe(45);
    expect(minutesLeft(FILM, at("20:45:01"))).toBe(45);
    expect(minutesLeft(FILM, at("20:45:59"))).toBe(45);
    expect(minutesLeft(FILM, at("20:30"))).toBe(60);
  });

  it("never reports less than one minute", () => {
    expect(minutesLeft(FILM, at("21:29:30"))).toBe(1);
    expect(minutesLeft(FILM, at("21:30"))).toBe(1);
    expect(minutesLeft(FILM, at("21:45"))).toBe(1);
  });
});

describe("elapsedFraction", () => {
  it("measures how far through a Programme an instant is", () => {
    expect(elapsedFraction(FILM, at("20:30"))).toBe(0);
    expect(elapsedFraction(FILM, at("20:45"))).toBe(0.25);
    expect(elapsedFraction(FILM, at("21:30"))).toBe(1);
  });

  it("stays between nothing and everything outside the Programme", () => {
    expect(elapsedFraction(FILM, at("19:00"))).toBe(0);
    expect(elapsedFraction(FILM, at("23:00"))).toBe(1);
  });
});

/** A Programme on 1 September 2026, between two UTC clock times. */
function programme(
  title: string,
  startsAt: string,
  endsAt: string,
): ProgrammeSlot {
  return {
    title,
    startsAt: instant(startsAt),
    endsAt: instant(endsAt),
  };
}

/** A UTC clock time on 1 September 2026, as `HH:MM` or `HH:MM:SS`. */
function at(clock: string): Date {
  return new Date(instant(clock));
}

function instant(clock: string): ProgrammeSlot["startsAt"] {
  const time = clock.length === 5 ? `${clock}:00` : clock;
  return clientSchemas.isoInstant.parse(`2026-09-01T${time}.000Z`);
}
