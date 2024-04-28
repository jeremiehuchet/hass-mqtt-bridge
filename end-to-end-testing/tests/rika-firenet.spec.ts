import { expect, test, testPlatform } from "../fixtures";

test.beforeAll(async () => {
  await testPlatform.up();
});

test.afterAll(async () => {
  await testPlatform.down();
});

test.beforeEach(async ({ onboardingPage, mqttSettingsPage, loginPage }) => {
  await onboardingPage.goto();
  await onboardingPage.onboardIfNeeded("Test User", "test", "test", "France");

  await loginPage.goto();
  await loginPage.login("test", "test");

  await mqttSettingsPage.goto();
  await mqttSettingsPage.setConfig("mosquitto", 1883, "hass", "hass");

  await expect
    .poll(() => testPlatform.countRegisteredEntities(/^sensor\.rika_/), {
      message:
        "it seems hass-mqtt-bridge didn't published Rika stoves entities after 10 seconds",
      timeout: 10000,
    })
    .toBeGreaterThan(10);
});

test.describe("Rika Firenet bridge", () => {
  test("should create stove devices", async ({ screenshots, devicesPage }) => {
    await devicesPage.goto();
    await devicesPage.hasDevice("Stove Stove 12345");
    await devicesPage.hasDevice("Stove Stove 333444");
  });

  test("should push device details", async ({
    screenshots,
    devicesPage,
    deviceInfoPage,
  }) => {
    await devicesPage.goto();
    await devicesPage.gotoDevice("Stove Stove 12345");
    await deviceInfoPage.hasInfo("RIKA", "DOMO", "Firmware: 2.28");
    await screenshots.fullPage("rika-device-page");
  });

  test("should push sensor values", async ({ devicesPage, deviceInfoPage }) => {
    await devicesPage.goto();
    await devicesPage.gotoDevice("Stove Stove 12345");
    await deviceInfoPage.hasEntities({
      "Room temperature": "19.6 °C",
      Status: "Standby",
      "Total consumption": "368 kg",
      "Total runtime": "297:00:00",
    });
  });

  test("should push diagnostic values", async ({
    devicesPage,
    deviceInfoPage,
  }) => {
    await devicesPage.goto();
    await devicesPage.gotoDevice("Stove Stove 12345");
    await deviceInfoPage.hasDiagnostics({
      "Flame temperature": "13 °C",
      "Ignition count": "121",
      "On/Off cycle count": "6",
      "Parameter error count 0": "1",
      "Parameter error count 1": "0",
      "Parameter error count 10": "0",
      "Parameter error count 11": "0",
      "Parameter error count 12": "0",
      "Parameter error count 13": "0",
      "Parameter error count 14": "0",
      "Parameter error count 15": "0",
      "Parameter error count 16": "0",
      "Parameter error count 17": "0",
      "Parameter error count 18": "0",
      "Parameter error count 19": "0",
      "Parameter error count 2": "0",
      "Parameter error count 3": "0",
      "Parameter error count 4": "0",
      "Parameter error count 5": "0",
      "Parameter error count 6": "0",
      "Parameter error count 7": "0",
      "Parameter error count 8": "0",
      "Parameter error count 9": "0",
      "Wifi strength": "-47 dBm",
    });
  });
});
