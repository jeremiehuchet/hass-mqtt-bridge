import { expect, type Locator, type Page } from "@playwright/test";

export class DevicesPage {
  private readonly page: Page;
  private readonly devicesList: Locator;

  constructor(page: Page) {
    this.page = page;
    this.devicesList = page.locator("ha-data-table").getByRole("row");
  }

  async goto() {
    await this.page.goto("/config/devices/dashboard", { waitUntil: "load" });
  }

  async hasDevice(deviceName: string) {
    await this.devicesList.getByText(deviceName).scrollIntoViewIfNeeded();
    await expect(this.devicesList.getByText(deviceName)).toBeVisible();
  }

  async gotoDevice(deviceName: string) {
    await this.devicesList
      .filter({
        hasText: deviceName,
      })
      .click();
  }
}
