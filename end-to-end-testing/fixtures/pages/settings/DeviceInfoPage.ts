import { Locator, Page, expect } from "@playwright/test";

export class DeviceInfoPage {
  private readonly page: Page;

  private readonly manufacturer: Locator;
  private readonly model: Locator;
  private readonly extra: Locator;

  private readonly entities: Locator;

  private readonly diagnostics: Locator;

  constructor(page: Page) {
    this.page = page;
    const info = page.locator("ha-device-info-card");
    this.manufacturer = info.locator(".manuf");
    this.model = info.locator(".model");
    this.extra = info.locator(".extra-info");
    this.entities = page.locator(
      "ha-device-entities-card:has-text('Sensors') hui-sensor-entity-row",
    );
    this.diagnostics = page.locator(
      "ha-device-entities-card:has-text('Diagnostic') hui-sensor-entity-row",
    );
  }

  async takeScreenshot(screenshotName: string) {
    this.page.screenshot({ path: `screenshots/${screenshotName}.png` });
  }

  async hasInfo(manufacturer: string, model: string, extra: string) {
    await expect(this.manufacturer).toHaveText(`by ${manufacturer}`);
    await expect(this.model).toHaveText(model);
    await expect(this.extra).toHaveText(extra);
  }

  async hasEntity(name: string, value: string) {
    await expect(
      this.entities.filter({ hasText: `${value} ${name}` }),
    ).toBeVisible({ timeout: 100000 });
  }

  async hasEntities(entities: { [key: string]: string }) {
    const expectedEntities = Object.entries(entities).map(
      ([name, value]) => `${value} ${name}`,
    );
    await expect(this.entities).toHaveText(expectedEntities);
  }

  async hasDiagnostics(diagnostics: { [key: string]: string }) {
    const expectedDiagnostics = Object.entries(diagnostics).map(
      ([name, value]) => `${value} ${name}`,
    );
    await expect(this.diagnostics).toHaveText(expectedDiagnostics);
  }

  async toggleSwitch(name: string) {
    await this.page.getByLabel(name).click();
  }
}
