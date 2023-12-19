import { test } from "../fixtures";
import { LoginPage } from "../fixtures/pages/LoginPage";
import { OnboardingPage } from "../fixtures/pages/OnboardingPage";
import { MqttSettingsPage } from "../fixtures/pages/settings/MqttSettingsPage";

test.beforeAll(async ({ browser }) => {
  const page = await browser.newPage();

  const onboardingPage = new OnboardingPage(page);
  await onboardingPage.goto();
  await onboardingPage.onboardIfNeeded("Test User", "test", "test", "France");

  const loginPage = new LoginPage(page);
  await loginPage.goto();
  await loginPage.login("test", "test");

  const mqttSettingsPage = new MqttSettingsPage(page);
  await mqttSettingsPage.goto();
  await mqttSettingsPage.setConfig("mosquitto", 1883, "hass", "hass");

  page.close();
});

test.beforeEach(async ({ loginPage }) => {
  await loginPage.goto();
  await loginPage.login("test", "test");
});

test.describe("Rika Firenet bridge", () => {
  test("should create stove devices", async ({ devicesPage }) => {
    await devicesPage.goto();
    await devicesPage.hasDevice("Stove Stove 12345");
    await devicesPage.hasDevice("Stove Stove 333444");
  });

  test("should push device details", async ({
    devicesPage,
    deviceInfoPage,
  }) => {
    await devicesPage.gotoDevice("Stove Stove 12345");
    await deviceInfoPage.hasInfo("RIKA", "DOMO", "Firmware: 228");
  });

  test("should push sensor values", async ({ devicesPage, deviceInfoPage }) => {
    await devicesPage.gotoDevice("Stove Stove 12345");
    await deviceInfoPage.hasEntities([
      "Room temperature 19.6°C",
      "Status Standby",
    ]);
  });

  test("should push diagnostic values", async ({
    devicesPage,
    deviceInfoPage,
  }) => {
    await devicesPage.gotoDevice("Stove Stove 12345");
    await deviceInfoPage.hasDiagnostics([
      "Flame temperature 13 °C",
      "Ignition count 121",
      "On/Off cycle count 6",
      "Parameter error count 0 1",
      "Total Consumption 368 kg",
      "Total runtime 297:00:00",
      "Wifi strength -47 dBm",
    ]);
  });
});
