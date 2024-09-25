import { Locator, Page, expect } from "@playwright/test";

export class ScreenshotsFixture {
  private readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  async fullPage(name: string) {
    await this.page.waitForLoadState("networkidle");
    await this.page.screenshot({
      path: `screenshots/${name}.png`,
      fullPage: true,
    });
  }

  async element(locator: string, name: string) {
    await this.page.waitForLoadState("networkidle");
    await this.page
      .locator(locator)
      .screenshot({ path: `screenshots/${name}.png` });
  }
}
