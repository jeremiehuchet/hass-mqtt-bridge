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
    this.entities = page.locator("ha-device-entities-card");
    this.diagnostics = page.locator("ha-device-diagnostics-card");
  }

  async hasInfo(manufacturer: string, model: string, extra: string) {
    await expect(this.manufacturer).toBe(manufacturer);
    await expect(this.model).toBe(model);
    await expect(this.extra).toBe(extra);
  }

  async hasEntities(entities: string[]) {
    for (const entity in entities) {
      await expect(this.entities).toContainText(entity);
    }
  }

  async hasDiagnostics(diagnostics: string[]) {
    for (const diagnostic in diagnostics) {
      await expect(this.diagnostics).toContainText(diagnostic);
    }
  }
}
