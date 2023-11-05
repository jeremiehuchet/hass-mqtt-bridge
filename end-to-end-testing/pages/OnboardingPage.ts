import { type Locator, type Page } from "@playwright/test";

export class OnboardingPage {
  private readonly page: Page;
  private readonly startButton: Locator;
  private readonly nameField: Locator;
  private readonly usernameField: Locator;
  private readonly passwordField: Locator;
  private readonly passwordConfirmationField: Locator;
  private readonly createAccountButton: Locator;
  private readonly skipLocationButton: Locator;
  private readonly countrySelect: Locator;
  private readonly submitCountryButton: Locator;
  private readonly skipStatisticsButton: Locator;
  private readonly finishOnboardingButton: Locator;

  constructor(page: Page) {
    this.page = page;
    this.startButton = page.getByRole("button", {
      name: "Create my smart home",
    });

    this.nameField = page.getByLabel("Name", { exact: true });
    this.usernameField = page.getByLabel("Username");
    this.passwordField = page.getByLabel("Password", { exact: true });
    this.passwordConfirmationField = page.getByLabel("Confirm password");
    this.createAccountButton = page.getByRole("button", {
      name: "Create account",
    });

    this.skipLocationButton = page.getByRole("button", { name: "Next" });

    this.countrySelect = page.getByLabel("Country");
    this.submitCountryButton = page.getByRole("button", { name: "Next" });

    this.skipStatisticsButton = page.getByRole("button", { name: "Next" });

    this.finishOnboardingButton = page.getByRole("button", { name: "Finish" });
  }

  async onboardIfNeeded(
    name: string,
    username: string,
    password: string,
    country: string,
  ) {
    await this.goto();
    const keepMeLoggedInCheckbox = this.page.getByLabel("Keep me logged in");
    await this.startButton.or(keepMeLoggedInCheckbox).waitFor();
    if (await this.startButton.isVisible()) {
      await this.start();
      await this.createAccount(name, username, password);
      await this.skipLocation();
      await this.selectCountry(country);
      await this.skipStatistics();
      await this.finish();
    }
  }

  async goto() {
    await this.page.context().clearCookies();
    await this.page.goto("/onboarding.html");
  }

  async start() {
    await this.startButton.click();
  }
  async createAccount(name: string, username: string, password: string) {
    await this.nameField.fill(name);
    await this.usernameField.fill(username);
    await this.passwordField.fill(password);
    await this.passwordConfirmationField.fill(password);
    await this.createAccountButton.click();
  }

  async skipLocation() {
    await this.skipLocationButton.click();
  }

  async selectCountry(country: string) {
    await this.countrySelect.click();
    await this.page.getByRole("option", { name: country }).click();
    await this.submitCountryButton.click();
  }

  async skipStatistics() {
    await this.skipStatisticsButton.click();
  }

  async finish() {
    await this.finishOnboardingButton.click();
  }
}
