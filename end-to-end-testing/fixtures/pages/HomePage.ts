import { expect, type Locator, type Page } from "@playwright/test";
import path from "path";

export class HomePage {
  constructor(private readonly page: Page) {
    this.page = page;
  }

  async goto() {
    await this.page.context().clearCookies();
    await this.page.goto("/", { waitUntil: "networkidle" });
  }

  async takeThermostatScreenshot(
    thermostatTitle: string,
    screenshotName: string,
  ) {
    await this.page
      .locator("hui-thermostat-card")
      .filter({ hasText: thermostatTitle })
      .screenshot({ path: `screenshots/${screenshotName}.png` });
  }
}
