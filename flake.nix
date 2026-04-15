{
  description = "Smart contract vulnerability detection - LLM dev env";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };

      in {
        devShells.default = pkgs.mkShell {
          name = "sc-llm";

          packages = with pkgs; [
            slither-analyzer
            solc-select
            foundry
            echidna
          ];
        };
      }
    );
}
