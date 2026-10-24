{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  libxkbcommon,
  grim,
  wf-recorder,
  ffmpeg,
  gifski,
  xdg-utils,
}:
rustPlatform.buildRustPackage {
  pname = "snapframe";
  version = (lib.importTOML ../Cargo.toml).package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../assets
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
  ];
  buildInputs = [ libxkbcommon ];

  # The capture helpers it drives; hyprctl comes from the running Hyprland.
  postFixup = ''
    wrapProgram $out/bin/snapframe --suffix PATH : ${
      lib.makeBinPath [
        grim
        wf-recorder
        ffmpeg
        gifski
        xdg-utils
      ]
    }
  '';

  meta = {
    description = "One-key screenshots, GIFs and screen recordings for Wayland";
    license = lib.licenses.mit;
    mainProgram = "snapframe";
    platforms = lib.platforms.linux;
  };
}
