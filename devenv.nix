{ inputs,  pkgs, ... }:

{
  packages = with pkgs; [
    docker
    openssl
    vscode

    playwright-driver.browsers
  ];

  languages.rust = {
    enable = true;
    channel = "stable";
  };
  languages.javascript.enable = true;
  languages.javascript.corepack.enable = true;
  languages.typescript.enable = true;

  pre-commit.hooks.rustfmt.enable = true;
  pre-commit.hooks.prettier.enable = true;

  scripts.end-to-end-tests.exec = ''
    cd $DEVENV_ROOT/end-to-end-testing
    pnpm install
    pnpm test
  '';

  enterShell = ''
    export PLAYWRIGHT_BROWSERS_PATH=${pkgs.playwright-driver.browsers}
    export PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=true
  '';
}
