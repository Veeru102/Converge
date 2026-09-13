import { expect, type Browser, type BrowserContext, type Page } from "@playwright/test";

export interface Hooks {
  hash: () => string;
  status: () => { state: string; lastSeq: string; pending: number; unacked: number; unsaved: number };
  objectCount: () => number;
  setOffline: (v: boolean) => void;
  flush: () => Promise<void>;
  docId: string;
}

declare global {
  interface Window {
    __converge: Hooks;
  }
}

export async function openTab(browser: Browser, docId: string, name: string): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext();
  const page = await context.newPage();
  if (process.env.E2E_DEBUG) page.on("console", (m) => console.log(`[${name}] ${m.text()}`));
  await page.goto(`/?doc=${docId}&name=${name}${process.env.E2E_DEBUG ? "&debug=1" : ""}`);
  await page.waitForFunction(() => !!window.__converge);
  return { context, page };
}

export async function newDocId(request: { post: (url: string) => Promise<{ json: () => Promise<unknown> }> }): Promise<string> {
  const res = await request.post("/docs");
  return ((await res.json()) as { id: string }).id;
}

export const hash = (page: Page) => page.evaluate(() => window.__converge.hash());
export const status = (page: Page) => page.evaluate(() => window.__converge.status());
export const objectCount = (page: Page) => page.evaluate(() => window.__converge.objectCount());

export async function waitLive(page: Page): Promise<void> {
  await expect.poll(async () => (await status(page)).state, { timeout: 20_000 }).toBe("live");
}

/** All pages agree with the server's durable state. */
export async function expectConverged(pages: Page[], baseURL: string, docId: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const res = await fetch(`${baseURL}/docs/${docId}/hash`);
        const server = (await res.json()) as { hash: string; durable_seq: number; head_seq: number };
        const states = await Promise.all(pages.map(async (p) => ({ hash: await hash(p), st: await status(p) })));
        const ok =
          server.durable_seq === server.head_seq &&
          states.every((s) => s.hash === server.hash && s.st.unacked === 0 && s.st.lastSeq === String(server.durable_seq));
        return ok ? "converged" : JSON.stringify({ server, states });
      },
      { timeout: 60_000, intervals: [250, 500, 1000] },
    )
    .toBe("converged");
}

export async function addRects(page: Page, n: number): Promise<void> {
  for (let i = 0; i < n; i++) await page.getByTestId("add-rect").click();
}

export async function dragFirstShape(page: Page, dx: number, dy: number): Promise<void> {
  const shape = page.getByTestId("shape").first();
  const box = (await shape.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  for (let i = 1; i <= 8; i++) await page.mouse.move(box.x + box.width / 2 + (dx * i) / 8, box.y + box.height / 2 + (dy * i) / 8);
  await page.mouse.up();
}

export async function setChaos(baseURL: string, cfg: Record<string, unknown>): Promise<void> {
  const res = await fetch(`${baseURL}/admin/chaos`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(cfg) });
  expect(res.ok).toBe(true);
}
