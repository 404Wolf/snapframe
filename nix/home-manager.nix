{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.snapframe;
  toml = pkgs.formats.toml { };
in
{
  options.programs.snapframe = {
    enable = lib.mkEnableOption "snapframe, one-key screenshots, GIFs and screen recordings";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.snapframe;
      defaultText = lib.literalExpression "snapframe.packages.\${system}.snapframe";
      description = "The snapframe package.";
    };

    settings = lib.mkOption {
      inherit (toml) type;
      default = { };
      example = lib.literalExpression ''
        {
          output_dir = "~/Screenshots";
          delay_secs = 3;
          gif_fps = 15;
          gif_max_width = 1200;
          gif_quality = 90;
          notify = true;
        }
      '';
      description = ''
        Written to {file}`$XDG_CONFIG_HOME/snapframe/config.toml`. Every key is
        optional; see the README for what each does.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    xdg.configFile."snapframe/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = toml.generate "snapframe-config.toml" cfg.settings;
    };
  };
}
