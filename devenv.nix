{ inputs,  pkgs, ... }:

{
  packages = [
    pkgs.docker-compose
    pkgs.openssl
    pkgs.vscode
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
    cd end-to-end-testing
    pnpm install
    pnpm test
  '';
}
