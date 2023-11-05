import { test } from "@playwright/test";
import { LoginPage } from "../pages/LoginPage";
import { DevicesPage } from "../pages/settings/DevicesPage";
import { MqttSettingsPage } from "../pages/settings/MqttSettingsPage";
import { OnboardingPage } from "../pages/OnboardingPage";

test.beforeEach(async ({ page }) => {
  const onboarding = new OnboardingPage(page);
  await onboarding.onboardIfNeeded("Test User", "test", "test", "France");

  const loginPage = new LoginPage(page);
  await loginPage.goto();
  await loginPage.login("test", "test");

  const mqttSettingsPage = new MqttSettingsPage(page);
  await mqttSettingsPage.goto();
  await mqttSettingsPage.setConfig("mosquitto", 1883, "hass", "hass");
});

test.describe("Rika Firenet device", () => {
  test("Display device sensors", async ({ page }) => {
    const devicesPage = new DevicesPage(page);
    await devicesPage.goto();
    await devicesPage.hasDevice("Home stove");
  });
});
