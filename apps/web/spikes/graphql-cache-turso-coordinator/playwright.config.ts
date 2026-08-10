import { fileURLToPath } from 'node:url';
import { defineConfig, devices } from '@playwright/test';

const spikeDirectory = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig({
  testDir: spikeDirectory,
  testMatch: 'browser.e2e.ts',
  timeout: 45_000,
  fullyParallel: false,
  workers: 1,
  reporter: 'line',
  use: {
    baseURL: 'http://127.0.0.1:4179',
    headless: true,
  },
  webServer: {
    command: `bunx --bun vite --config ${JSON.stringify(
      `${spikeDirectory}/vite.config.ts`
    )}`,
    url: 'http://127.0.0.1:4179',
    reuseExistingServer: false,
    timeout: 30_000,
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
  ],
});
