import { type Locator, type Page } from "@playwright/test";

export class LoginPage {
  private readonly page: Page;
  private readonly usernameField: Locator;
  private readonly passwordField: Locator;
  private readonly rememberMeCheckbox: Locator;
  private readonly loginButton: Locator;

  constructor(page: Page) {
    this.page = page;
    this.usernameField = page.getByLabel("Username");
    this.passwordField = page.getByLabel("Password", { exact: true });
    this.rememberMeCheckbox = page.getByLabel("Keep me logged in");
    this.loginButton = page.getByRole("button", { name: "Log in" });
  }

  async goto() {
    await this.page.context().clearCookies();
    await this.page.goto("/");
  }

  async login(username: string, password: string) {
    await this.rememberMeCheckbox.check();
    await this.usernameField.fill(username);
    await this.passwordField.fill(password);
    await this.passwordField.press("Enter");
    await this.page.waitForURL("/lovelace/0");
  }
}
