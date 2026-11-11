import { defineConfig, devices } from '@playwright/test';

// The UI dev server runs in the `ui` service of compose.yaml; the test
// container shares its network namespace, so it is always localhost:5173.
const baseURL = process.env.BASE_URL ?? 'http://localhost:5173/';

export default defineConfig({
  testDir: './tests',
  outputDir: './test-results',
  timeout: 30_000,
  expect: { timeout: 7_000 },
  fullyParallel: true,
  retries: 0,
  reporter: [['list'], ['html', { outputFolder: 'playwright-report', open: 'never' }]],
  use: {
    baseURL,
    locale: 'he-IL',
    timezoneId: 'Asia/Jerusalem',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    // Must stay green on main. Onboarding through sending a message, and channels.
    { name: 'smoke', testMatch: /(smoke|channels|language)\.spec\.ts/, use: { ...devices['Desktop Chrome'] } },
    // Exploratory checks and bug repros from BUGS.md. Known bugs are marked
    // test.fail(), so the run stays green until a bug is fixed – then the
    // test "unexpectedly passes" and the mark should be removed.
    { name: 'explore', testMatch: /explore\.spec\.ts/, use: { ...devices['Desktop Chrome'] } },
  ],
});
