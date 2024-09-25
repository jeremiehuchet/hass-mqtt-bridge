import { test as base } from "@playwright/test";
import { OnboardingPage } from "./pages/OnboardingPage";
import { LoginPage } from "./pages/LoginPage";
import { HomePage } from "./pages/HomePage";
import { DevicesPage } from "./pages/settings/DevicesPage";
import { MqttSettingsPage } from "./pages/settings/MqttSettingsPage";
import { DeviceInfoPage } from "./pages/settings/DeviceInfoPage";
import { ScreenshotsFixture } from "./screenshots";
import { TestPlatform } from "./test-platform";

type Fixtures = {
  platform: TestPlatform;
  screenshots: ScreenshotsFixture;
  onboardingPage: OnboardingPage;
  loginPage: LoginPage;
  homePage: HomePage;
  mqttSettingsPage: MqttSettingsPage;
  devicesPage: DevicesPage;
  deviceInfoPage: DeviceInfoPage;
};

export const testPlatform = new TestPlatform();

export const test = base.extend<Fixtures>({
  screenshots: async ({ page }, use) => {
    await use(new ScreenshotsFixture(page));
  },
  onboardingPage: async ({ page }, use) => {
    await use(new OnboardingPage(page));
  },
  loginPage: async ({ page }, use) => {
    await use(new LoginPage(page));
  },
  homePage: async ({ page }, use) => {
    await use(new HomePage(page));
  },
  mqttSettingsPage: async ({ page }, use) => {
    await use(new MqttSettingsPage(page));
  },
  devicesPage: async ({ page }, use) => {
    await use(new DevicesPage(page));
  },
  deviceInfoPage: async ({ page }, use) => {
    await use(new DeviceInfoPage(page));
  },
});
export { expect } from "@playwright/test";
