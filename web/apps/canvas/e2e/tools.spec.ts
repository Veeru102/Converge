import { expect, test, type Page } from "@playwright/test";
import { drawShape, expectConverged, newDocId, objectCount, openTab, selectFirstShape, setChaos, status, waitLive } from "./helpers.js";

const kinds = (page: Page) => page.getByTestId("shape").evaluateAll((els) => els.map((e) => e.getAttribute("data-kind")));
const firstBox = async (page: Page) => (await page.getByTestId("shape").first().boundingBox())!;

test.afterEach(async ({ baseURL }) => {
  await setChaos(baseURL!, { enabled: false });
});

test("every tool draws, and both tabs render the same objects", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  const b = await openTab(browser, doc, "bob");
  await waitLive(a.page);
  await waitLive(b.page);
  await drawShape(a.page, "rect", [150, 150], [300, 250]);
  await drawShape(a.page, "ellipse", [400, 150], [520, 260]);
  await drawShape(a.page, "line", [300, 200], [400, 205]);
  await a.page.getByTestId("tool-text").click();
  await a.page.mouse.click(160, 400);
  await expect(a.page.getByTestId("text-editor")).toBeFocused();
  await a.page.keyboard.type("Hello, Converge");
  await a.page.keyboard.press("Enter");
  await expect.poll(() => kinds(a.page)).toEqual(["rect", "ellipse", "line", "text"]);
  await expect.poll(() => kinds(b.page)).toEqual(["rect", "ellipse", "line", "text"]);
  await expect(b.page.locator("text", { hasText: "Hello, Converge" })).toBeVisible();
  // The tool returns to select after one use.
  await expect(a.page.getByTestId("tool-select")).toHaveAttribute("aria-pressed", "true");
  await expectConverged([a.page, b.page], baseURL!, doc);
  await a.context.close();
  await b.context.close();
});

test("resize handles, duplicate, delete and z-order", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  await waitLive(a.page);
  await drawShape(a.page, "rect", [150, 150], [250, 230]);
  await selectFirstShape(a.page);
  await expect(a.page.getByTestId("properties")).toBeVisible();
  // Drag the south-east handle.
  const before = await firstBox(a.page);
  const se = (await a.page.locator("[data-handle=se]").boundingBox())!;
  await a.page.mouse.move(se.x + se.width / 2, se.y + se.height / 2);
  await a.page.mouse.down();
  await a.page.mouse.move(se.x + 80, se.y + 50, { steps: 6 });
  await a.page.mouse.up();
  const after = await firstBox(a.page);
  expect(after.width).toBeGreaterThan(before.width + 60);
  expect(after.height).toBeGreaterThan(before.height + 30);
  // Duplicate → 2 objects, the copy is selected and offset.
  await a.page.keyboard.press("Meta+d");
  await expect.poll(() => objectCount(a.page)).toBe(2);
  const boxes = await a.page.getByTestId("shape").evaluateAll((els) => els.map((e) => e.getBoundingClientRect().x));
  expect(boxes[1]! - boxes[0]!).toBeCloseTo(16, 0);
  // Z-order: send the (selected) copy to the back → it renders first.
  await a.page.getByTestId("z-back").click();
  await expect.poll(async () => (await a.page.getByTestId("shape").evaluateAll((els) => els.map((e) => e.getBoundingClientRect().x)))[0]).toBeCloseTo(boxes[1]!, 0);
  // Delete via keyboard.
  await a.page.keyboard.press("Delete");
  await expect.poll(() => objectCount(a.page)).toBe(1);
  await expectConverged([a.page], baseURL!, doc);
  await a.context.close();
});

test("marquee multi-select moves several objects and shows in the other tab's presence", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  const b = await openTab(browser, doc, "bob");
  await waitLive(a.page);
  await waitLive(b.page);
  await drawShape(a.page, "rect", [150, 150], [230, 210]);
  await drawShape(a.page, "ellipse", [300, 150], [380, 210]);
  await a.page.getByTestId("tool-select").click();
  await a.page.mouse.move(100, 100);
  await a.page.mouse.down();
  await a.page.mouse.move(420, 260, { steps: 5 });
  await a.page.mouse.up();
  await expect(a.page.getByTestId("properties").locator("header")).toContainText("2 objects");
  // Bob sees ann's selection outlines.
  await expect.poll(() => b.page.locator("svg.canvas g[pointer-events=none] rect[stroke]").count()).toBeGreaterThanOrEqual(2);
  const xs = async () => a.page.getByTestId("shape").evaluateAll((els) => els.map((e) => e.getBoundingClientRect().x));
  const start = await xs();
  const box = await firstBox(a.page);
  await a.page.mouse.move(box.x + 20, box.y + 20);
  await a.page.mouse.down();
  await a.page.mouse.move(box.x + 120, box.y + 60, { steps: 6 });
  await a.page.mouse.up();
  const end = await xs();
  expect(end[0]! - start[0]!).toBeCloseTo(100, 0);
  expect(end[1]! - start[1]!).toBeCloseTo(100, 0);
  await expectConverged([a.page, b.page], baseURL!, doc);
  await a.context.close();
  await b.context.close();
});

test("network lab: presets drive server chaos, offline toggle queues ops, badge returns to converged", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  await waitLive(a.page);
  await a.page.getByTestId("lab-toggle").click();
  await expect(a.page.getByTestId("lab")).toBeVisible();
  await a.page.getByTestId("preset-hostile").click();
  await expect.poll(async () => ((await (await fetch(`${baseURL}/admin/chaos`)).json()) as { enabled: boolean; drop_p: number }).drop_p).toBe(0.3);
  await a.page.getByTestId("preset-clean").click();
  await expect.poll(async () => ((await (await fetch(`${baseURL}/admin/chaos`)).json()) as { enabled: boolean }).enabled).toBe(false);
  await a.page.getByTestId("offline-toggle").check();
  await expect(a.page.getByTestId("status")).toHaveAttribute("data-state", "offline");
  await drawShape(a.page, "rect", [150, 150], [250, 230]);
  await expect.poll(async () => (await status(a.page)).pending).toBeGreaterThan(0);
  await expect(a.page.getByTestId("stat-pending")).not.toHaveText("0");
  await a.page.getByTestId("offline-toggle").uncheck();
  await expect(a.page.getByTestId("converged-badge")).toContainText("Converged");
  await expect(a.page.getByTestId("hash-match")).toHaveText("match");
  await expectConverged([a.page], baseURL!, doc);
  // In-app benchmark: a burst of 200 ops commits and reports latency samples.
  await a.page.getByTestId("bench-burst").click();
  await expect(a.page.getByTestId("bench-result")).toContainText("200 ops committed", { timeout: 30_000 });
  await expect(a.page.getByTestId("bench-latency")).not.toContainText("—");
  await a.page.getByTestId("bench-offline-burst").click();
  await expect(a.page.getByTestId("bench-result")).toContainText("queued offline and committed", { timeout: 30_000 });
  await expectConverged([a.page], baseURL!, doc);
  await a.context.close();
});
