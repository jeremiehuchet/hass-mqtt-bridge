import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: [
    ["github"],
    ["list", { printSteps: true }],
    ["html", { open: "never" }],
    ["junit", { outputFile: "playwright-report/results.xml" }],
  ],
  use: {
    baseURL: "http://127.0.0.1:8123",
    trace: "on-first-retry",
    video: "on",
    screenshot: "on",
    headless: process.env.CI ? true : false,
  },
  timeout: 2 * 60 * 1000,
  expect: {
    timeout: 10 * 1000,
  },
  projects: [
    {
      name: "firefox",
      use: {
        ...devices["Desktop Firefox"],
      },
    },
  ],
});
