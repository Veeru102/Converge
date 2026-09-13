import { defineConfig } from "@playwright/test";

const PORT = 8090;

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: { baseURL: `http://127.0.0.1:${PORT}`, trace: "retain-on-failure" },
  webServer: {
    // Build the app, then run the Rust server serving it (in-memory storage unless DATABASE_URL is set).
    command: `pnpm vite build && cd ../../.. && cargo run --release -q -p converge-server`,
    url: `http://127.0.0.1:${PORT}/healthz`,
    reuseExistingServer: !process.env.CI,
    timeout: 600_000,
    env: {
      BIND: `127.0.0.1:${PORT}`,
      STATIC_DIR: `${process.cwd()}/dist`,
      PERSIST_WINDOW_MS: "5",
      SNAPSHOT_EVERY: "50",
      CHAOS_SEED: "42",
      RUST_LOG: "info",
    },
  },
});
