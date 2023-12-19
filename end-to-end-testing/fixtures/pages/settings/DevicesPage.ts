import { expect, type Locator, type Page } from "@playwright/test";

export class DevicesPage {
  private readonly page: Page;
  private readonly devicesList: Locator;

  constructor(page: Page) {
    this.page = page;
    this.devicesList = page.locator("ha-data-table").getByRole("row");
  }

  async goto() {
    await this.page.goto("/config/devices/dashboard");
  }

  async hasDevice(deviceName: string) {
    const matchingDevicesLocator = this.devicesList.filter({
      hasText: deviceName,
    });
    await expect(matchingDevicesLocator).toHaveCount(1);
  }

  async gotoDevice(deviceName: string) {
    this.devicesList
      .filter({
        hasText: deviceName,
      })
      .click();
  }
}
