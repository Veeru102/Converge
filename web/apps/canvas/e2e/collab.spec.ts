import { expect, test } from "@playwright/test";
import { addRects, dragFirstShape, expectConverged, newDocId, objectCount, openTab, setChaos, status, waitLive } from "./helpers.js";

test.afterEach(async ({ baseURL }) => {
  await setChaos(baseURL!, { enabled: false });
});

test("two tabs edit the same document live and converge with the server", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  const b = await openTab(browser, doc, "bob");
  await waitLive(a.page);
  await waitLive(b.page);
  await addRects(a.page, 3);
  await addRects(b.page, 2);
  await expect.poll(() => objectCount(a.page)).toBe(5);
  await expect.poll(() => objectCount(b.page)).toBe(5);
  await dragFirstShape(a.page, 120, 60);
  await dragFirstShape(b.page, -40, 90);
  // Recolor renders locally and on the other tab.
  const firstRect = (page: typeof a.page) => page.getByTestId("shape").first().locator("rect");
  const before = await firstRect(a.page).getAttribute("fill");
  await a.page.getByTestId("shape").first().click();
  await a.page.getByTestId("recolor").click();
  await expect(firstRect(a.page)).not.toHaveAttribute("fill", before!);
  await expect(firstRect(b.page)).toHaveAttribute("fill", (await firstRect(a.page).getAttribute("fill"))!);
  await expectConverged([a.page, b.page], baseURL!, doc);
  // Presence: each tab sees the other user.
  await expect(a.page.getByText("● bob")).toBeVisible();
  await expect(b.page.getByText("● ann")).toBeVisible();
  await a.context.close();
  await b.context.close();
});

test("offline edits are kept locally, survive a reload, and merge on reconnect", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  const b = await openTab(browser, doc, "bob");
  await waitLive(a.page);
  await waitLive(b.page);
  await addRects(a.page, 1);
  await expect.poll(() => objectCount(b.page)).toBe(1);

  // B goes offline and keeps editing; A edits concurrently.
  await b.page.evaluate(() => window.__converge.setOffline(true));
  await expect.poll(async () => (await status(b.page)).state).not.toBe("live");
  await addRects(b.page, 3);
  await dragFirstShape(b.page, 200, 0);
  await addRects(a.page, 2);
  await expect.poll(() => objectCount(a.page)).toBe(3);
  await expect.poll(() => objectCount(b.page)).toBe(4);
  await expect.poll(async () => (await status(b.page)).unacked).toBeGreaterThan(0);

  // Reload B while its network to the server is down: state comes back from
  // IndexedDB (snapshot ∪ pending). Only the socket is blocked so the page
  // itself can load.
  await b.page.evaluate(() => window.__converge.flush());
  await b.page.goto(`/?doc=${doc}&name=bob&offline=1`);
  await b.page.waitForFunction(() => !!window.__converge);
  await expect.poll(() => objectCount(b.page)).toBe(4);
  await expect.poll(async () => (await status(b.page)).unacked).toBeGreaterThan(0);
  expect((await status(b.page)).state).not.toBe("live");

  // Back online: B's queued ops flush, both tabs converge to 6 objects.
  await b.page.evaluate(() => window.__converge.setOffline(false));
  await expect.poll(() => objectCount(a.page), { timeout: 30_000 }).toBe(6);
  await expect.poll(() => objectCount(b.page), { timeout: 30_000 }).toBe(6);
  await expectConverged([a.page, b.page], baseURL!, doc);
  await a.context.close();
  await b.context.close();
});

test("three tabs converge under server-side chaos (latency, drops, duplicates, disconnects)", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const tabs = await Promise.all(["ann", "bob", "cy"].map((n) => openTab(browser, doc, n)));
  for (const t of tabs) await waitLive(t.page);
  await setChaos(baseURL!, { enabled: true, latency_ms: [5, 120], drop_p: 0.15, dup_p: 0.15, disconnect_every: 20 });
  for (let round = 0; round < 6; round++) {
    for (const t of tabs) {
      await addRects(t.page, 1);
      if (round % 2 === 0) await dragFirstShape(t.page, 15, 10);
    }
  }
  await setChaos(baseURL!, { enabled: false });
  await expectConverged(
    tabs.map((t) => t.page),
    baseURL!,
    doc,
  );
  for (const t of tabs) await expect.poll(() => objectCount(t.page)).toBe(18);
  for (const t of tabs) await t.context.close();
});

test("a tab that reconnects after the server closed it resumes by sequence", async ({ browser, request, baseURL }) => {
  const doc = await newDocId(request);
  const a = await openTab(browser, doc, "ann");
  await waitLive(a.page);
  await addRects(a.page, 4);
  await expectConverged([a.page], baseURL!, doc);
  // Force server-side disconnects on every message for a while.
  await setChaos(baseURL!, { enabled: true, disconnect_every: 2 });
  await addRects(a.page, 4);
  await setChaos(baseURL!, { enabled: false });
  await expectConverged([a.page], baseURL!, doc);
  await expect.poll(() => objectCount(a.page)).toBe(8);
  await a.context.close();
});
