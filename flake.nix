{
  description = "snapframe: one-key screenshots, GIFs and screen recordings for Wayland (Hyprland/wlroots)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    {
      overlays.default = final: _prev: {
        snapframe = final.callPackage ./nix/package.nix { };
      };
      homeManagerModules.default = import ./nix/home-manager.nix { inherit self; };
    }
    // flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ self.overlays.default ];
        };
      in
      {
        packages = {
          inherit (pkgs) snapframe;
          default = pkgs.snapframe;
        };
        checks = {
          package = pkgs.snapframe;
          clippy = pkgs.snapframe.overrideAttrs (old: {
            pname = "snapframe-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = ''
              runHook preBuild
              cargo clippy --offline --all-targets -- -D warnings
              runHook postBuild
            '';
            doCheck = false;
            installPhase = "touch $out";
            postFixup = "";
          });
        };
        devShells.default = pkgs.mkShell {
          inputsFrom = [ pkgs.snapframe ];
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
          ];
        };
        formatter = pkgs.nixfmt-tree;
      }
    );
}
