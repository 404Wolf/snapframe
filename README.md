# snapframe

One key for screenshots, GIFs and screen recordings on Wayland (Hyprland and
other wlroots compositors).

Press the bind and every output freezes under a dimmed overlay with a small
toolbar:

- **Drag** to select a region, **click** a window to take it, **double-click**
  or press **F** for the whole output. Adjust the selection with its handles,
  or drag it to move it.
- **S** / **G** / **V** (or the toolbar) pick Image, GIF or Video. **D** toggles a
  delay (3 s by default).
- **Enter** captures, **Esc** (or right-click) cancels.

An image is cropped from the frozen frame, so it is exactly what was on screen
when you pressed the bind. GIFs and videos record the selection with a small
timer pill on screen (kept out of the recorded area where it fits); click it, or
press the bind again, to stop. Captures are saved to `~/Screenshots`, put on the
clipboard, and announced with a notification that can open the file or folder.

## Clipboard

The capture's own bytes come first, so apps that paste images or video take
the media rather than a path:

| Capture | Offered types |
| --- | --- |
| PNG | `image/png`, `text/uri-list` |
| GIF | `image/gif`, `text/uri-list`, `text/plain` (the path) |
| MP4 | `video/mp4`, `text/uri-list`, `text/plain` (the path) |

A PNG deliberately offers no `text/plain`, so a terminal or chat box that asks
for text first can't paste the path instead of the picture. The clipboard is
served by a small detached `snapframe serve` process until something else is
copied.

## Command line

```sh
snapframe                                   # the overlay
snapframe --mode gif                        # the overlay, starting in GIF mode
snapframe --region 100,200,640,480          # capture a region, no overlay
snapframe --region "100,200 640x480"        # slurp's format works too
snapframe --mode video --region 0,0,1920,1080 --duration 5
snapframe stop                              # stop a recording / close the overlay
snapframe copy ~/Screenshots/shot.png       # put a saved capture on the clipboard
```

Running `snapframe` while another is running stops that one instead, so one
bind starts and stops recordings. Only one instance runs at a time (a socket in
`$XDG_RUNTIME_DIR`).

## Configuration

`$XDG_CONFIG_HOME/snapframe/config.toml`; every key is optional:

```toml
output_dir = "~/Screenshots"
delay_secs = 3      # what the delay toggle waits
gif_fps = 15
gif_max_width = 1200
gif_quality = 90    # gifski, 1 to 100
notify = true
```

## Nix

```nix
# flake.nix
inputs.snapframe.url = "github:404Wolf/snapframe";

# Home Manager
imports = [ inputs.snapframe.homeManagerModules.default ];
programs.snapframe = {
  enable = true;
  settings.delay_secs = 5;
};
wayland.windowManager.hyprland.settings.bind = [ "SUPER SHIFT, S, exec, snapframe" ];
```

The package wraps `grim`, `wf-recorder`, `ffmpeg`, `gifski` and `xdg-open`.
Window selection asks `hyprctl`; without Hyprland, regions and outputs still work.

## How it works

- One fullscreen `wlr-layer-shell` overlay surface per output, software-rendered
  (tiny-skia) into `wl_shm` buffers at the output's physical size and shown at its
  logical size through `wp_viewporter`, so fractional scales stay sharp and the
  crop maps exactly.
- Frozen frames come from `grim -o <output>`; recordings from `wf-recorder`; GIFs
  from `ffmpeg` piped into `gifski`.
- The overlay is unmapped, and the compositor has caught up, before any fresh
  capture or recording starts, so it never appears in a capture.

## Development

```sh
nix develop
cargo test
cargo clippy --all-targets -- -D warnings
nix flake check
```

## License

MIT. The bundled DejaVu Sans font is under its own license
(`assets/LICENSE-DejaVu`).
