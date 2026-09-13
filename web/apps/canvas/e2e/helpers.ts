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

export async function openTab(browser: Browser, docId: string, name: string, extra = ""): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  if (process.env.E2E_DEBUG) page.on("console", (m) => console.log(`[${name}] ${m.text()}`));
  page.on("pageerror", (e) => console.log(`[${name}] pageerror ${e.message}`));
  await page.goto(`/?doc=${docId}&name=${name}${process.env.E2E_DEBUG ? "&debug=1" : ""}${extra}`);
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

/** Draw `n` rectangles by drag at staggered positions (world == screen at 100 %). */
export async function addRects(page: Page, n: number, origin: [number, number] = [120, 120]): Promise<void> {
  for (let i = 0; i < n; i++) {
    await page.getByTestId("tool-rect").click();
    const x = origin[0] + (i % 5) * 150, y = origin[1] + Math.floor(i / 5) * 120;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 100, y + 70, { steps: 4 });
    await page.mouse.up();
  }
  await page.keyboard.press("Escape");
}

export async function drawShape(page: Page, tool: "rect" | "ellipse" | "line", from: [number, number], to: [number, number]): Promise<void> {
  await page.getByTestId(`tool-${tool}`).click();
  await page.mouse.move(from[0], from[1]);
  await page.mouse.down();
  await page.mouse.move(to[0], to[1], { steps: 6 });
  await page.mouse.up();
}

export async function dragFirstShape(page: Page, dx: number, dy: number): Promise<void> {
  await page.getByTestId("tool-select").click();
  const shape = page.getByTestId("shape").first();
  const box = (await shape.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  for (let i = 1; i <= 8; i++) await page.mouse.move(box.x + box.width / 2 + (dx * i) / 8, box.y + box.height / 2 + (dy * i) / 8);
  await page.mouse.up();
}

export async function selectFirstShape(page: Page): Promise<void> {
  await page.getByTestId("tool-select").click();
  const box = (await page.getByTestId("shape").first().boundingBox())!;
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
}

export async function setChaos(baseURL: string, cfg: Record<string, unknown>): Promise<void> {
  const res = await fetch(`${baseURL}/admin/chaos`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(cfg) });
  expect(res.ok).toBe(true);
}
