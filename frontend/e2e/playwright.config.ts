import { chromiumArgs, keelBaseUrl } from '@globalart/keel/testing/playwright';
import { defineConfig, devices } from '@playwright/test';

const ci = process.env['CI'] !== undefined;

export default defineConfig({
  testDir: '.',
  outputDir: 'test-results',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: ci,
  reporter: 'list',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: keelBaseUrl(),
    locale: 'en-US',
    timezoneId: 'UTC',
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'], launchOptions: { args: chromiumArgs() } },
    },
  ],
});
