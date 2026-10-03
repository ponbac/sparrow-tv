import { afterEach, describe, expect, it } from "vitest";
import { isPictureTap, stageMonitorSize } from "./stage-dom";

afterEach(() => {
  document.body.replaceChildren();
});

describe("isPictureTap", () => {
  it("takes a click on the picture, and none on a control inside or outside its box", () => {
    document.body.innerHTML = `
      <div id="monitor">
        <section><video></video><button id="inline"><span id="label">Pause</span></button></section>
      </div>
      <div id="slot"><button id="portalled">Pause</button></div>
    `;
    const monitor = byId("monitor");

    expect(isPictureTap(monitor, byId("monitor"))).toBe(true);
    expect(isPictureTap(monitor, document.querySelector("video"))).toBe(true);
    // The fullscreen player keeps its controls inside the box.
    expect(isPictureTap(monitor, byId("inline"))).toBe(false);
    expect(isPictureTap(monitor, byId("label"))).toBe(false);
    // What the player portals into the info block reaches the box only
    // through React, whether it is a control or the space between them.
    expect(isPictureTap(monitor, byId("portalled"))).toBe(false);
    expect(isPictureTap(monitor, byId("slot"))).toBe(false);
    expect(isPictureTap(monitor, null)).toBe(false);
  });
});

describe("stageMonitorSize", () => {
  it("measures the picture box, and nothing where there is none or it is not laid out", () => {
    expect(stageMonitorSize()).toBeNull();

    document.body.innerHTML = `<div class="stage__monitor"></div>`;
    const monitor = document.querySelector<HTMLElement>(".stage__monitor");
    if (monitor === null) {
      throw new Error("expected the picture box");
    }
    // jsdom lays nothing out: every box is empty, as one out of the layout is.
    expect(stageMonitorSize()).toBeNull();

    monitor.getBoundingClientRect = () =>
      DOMRect.fromRect({ x: 0, y: 38, width: 360, height: 202.5 });
    expect(stageMonitorSize()).toEqual({ width: 360, height: 202.5 });
  });
});

function byId(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`expected #${id}`);
  }
  return element;
}
