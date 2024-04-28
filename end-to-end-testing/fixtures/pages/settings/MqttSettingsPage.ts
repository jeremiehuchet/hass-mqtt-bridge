import { Locator, Page, expect } from "@playwright/test";
import { testPlatform } from "../..";

export class MqttSettingsPage {
  private readonly page: Page;
  private readonly addEntryButton: Locator;

  private readonly hostField: Locator;
  private readonly portField: Locator;
  private readonly usernameField: Locator;
  private readonly passwordField: Locator;
  private readonly submitButton: Locator;
  private readonly finishButton: Locator;
  private readonly closeButton: Locator;

  constructor(page: Page) {
    this.page = page;
    this.addEntryButton = page.getByRole("button", { name: "Add entry" });
    const dialogAddEntryFlow = page.locator("dialog-data-entry-flow");
    this.hostField = dialogAddEntryFlow.getByLabel("Broker");
    this.portField = dialogAddEntryFlow.getByLabel("Port");
    this.usernameField = dialogAddEntryFlow.getByLabel("Username");
    this.passwordField = dialogAddEntryFlow.getByLabel("Password", {
      exact: true,
    });
    this.submitButton = dialogAddEntryFlow.getByRole("button", {
      name: "Submit",
    });
    this.finishButton = dialogAddEntryFlow.getByRole("button", {
      name: "Finish",
    });
    this.closeButton = dialogAddEntryFlow.getByRole("button", {
      name: "Close",
    });
  }

  async goto() {
    await this.page.goto("/config/integrations/integration/mqtt");
  }

  async setConfig(
    host: string,
    port: number,
    username: string,
    password: string,
  ) {
    await this.addEntryButton.click();

    await expect(this.closeButton.or(this.submitButton)).toBeVisible();
    if (await this.closeButton.isVisible()) {
      await this.closeButton.click();
    } else {
      await this.hostField.fill(host);
      await this.portField.fill(port.toString());
      await this.usernameField.fill(username);
      await this.passwordField.fill(password);
      await this.submitButton.click();

      await this.finishButton.click();
      await expect
        .poll(() => testPlatform.getMosquittoMessages(), {
          message:
            "homeassistant should have sent an 'online' message to MQTT broker",
          timeout: 10000,
        })
        .toContain("homeassistant/status online");
    }
  }
}
