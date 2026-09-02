import { useEffect, useState } from "react";

const CLOCK_TICK_MS = 30_000;

/** Keeps visible clock and playhead copy current without rebuilding every second. */
export function useGuideClock(): Date {
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), CLOCK_TICK_MS);
    return () => window.clearInterval(timer);
  }, []);

  return now;
}
