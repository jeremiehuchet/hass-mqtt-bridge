{ inputs,  pkgs, ... }:

{
  packages = [
    pkgs.vscode
  ];

  languages.rust = {
    enable = true;
    channel = "stable";
  };

  pre-commit.hooks.rustfmt.enable = true;
}
