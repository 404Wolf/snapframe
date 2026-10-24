//! snapframe: one key for screenshots, GIFs and screen recordings on Wayland.

mod capture;
mod clip;
mod config;
mod geom;
mod hypr;
mod ipc;
mod notify;
mod record;
mod selection;
mod ui;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::config::Config;
use crate::geom::{PixelRect, Rect};
use crate::ipc::{Claim, Instance};
use crate::selection::Mode;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// What to capture; the overlay starts in this mode.
    #[arg(long, value_enum, default_value_t = ModeArg::Image)]
    mode: ModeArg,

    /// Skip the overlay and capture this region, `x,y,w,h` or `x,y wxh` in
    /// global logical pixels (what slurp prints).
    #[arg(long)]
    region: Option<String>,

    /// Wait this many seconds first (with --region; the overlay has its own toggle).
    #[arg(long)]
    delay: Option<u64>,

    /// With --region, stop a GIF or video recording after this many seconds.
    #[arg(long)]
    duration: Option<f64>,

    /// Save to this directory instead of the configured one.
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Don't put the capture on the clipboard.
    #[arg(long)]
    no_clipboard: bool,

    /// Don't show a notification.
    #[arg(long)]
    no_notify: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Stop the running recording, or close the overlay, if any.
    Stop,
    /// Put a saved capture (.png, .gif or .mp4) on the clipboard and serve it.
    Copy { path: PathBuf },
    #[command(hide = true)]
    Serve { path: PathBuf },
    #[command(hide = true)]
    Notify {
        path: PathBuf,
        #[arg(long)]
        copied: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModeArg {
    Image,
    Gif,
    Video,
}

impl From<ModeArg> for Mode {
    fn from(m: ModeArg) -> Mode {
        match m {
            ModeArg::Image => Mode::Image,
            ModeArg::Gif => Mode::Gif,
            ModeArg::Video => Mode::Video,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("snapframe: {error:#}");
        notify::message("Capture failed", &format!("{error:#}"));
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Some(Cmd::Serve { path }) => return clip::serve(path),
        Some(Cmd::Notify { path, copied }) => return notify::saved(path, *copied),
        Some(Cmd::Copy { path }) => return spawn_detached(&["serve".as_ref(), path.as_os_str()]),
        Some(Cmd::Stop) => {
            match ipc::send(&ipc::socket_path(), ipc::TOGGLE) {
                Ok(_) => {}
                Err(_) => eprintln!("snapframe: nothing is running"),
            }
            return Ok(());
        }
        None => {}
    }

    let instance = match ipc::claim(ipc::TOGGLE)? {
        Claim::Owned(instance) => instance,
        // Pressing the bind again stops what's running.
        Claim::Forwarded => return Ok(()),
    };
    let mut config = Config::load()?;
    if let Some(dir) = &cli.output_dir {
        config.output_dir = dir.clone();
    }
    if cli.no_notify {
        config.notify = false;
    }
    let mode = Mode::from(cli.mode);

    let saved = match &cli.region {
        Some(region) => {
            let region = PixelRect::parse(region).context("--region wants x,y,w,h")?;
            if let Some(secs) = cli.delay {
                std::thread::sleep(Duration::from_secs(secs));
            }
            capture_region(&instance, &config, mode, &region, None, cli.duration)?
        }
        None => match interactive(&instance, &config, mode)? {
            Some(path) => path,
            None => return Ok(()),
        },
    };
    finish(&config, &saved, !cli.no_clipboard)
}

fn interactive(instance: &Instance, config: &Config, mode: Mode) -> Result<Option<PathBuf>> {
    let (shots, selection) = ui::select(instance, mode, config.delay_secs)?;
    let ui::Selection::Confirmed {
        rect,
        output,
        mode,
        delay,
    } = selection
    else {
        return Ok(None);
    };
    let shot = &shots[output];
    if delay {
        std::thread::sleep(Duration::from_secs(config.delay_secs));
    }
    if mode == Mode::Image && !delay {
        // The frozen frame is exactly what was on screen when the bind was pressed.
        let crop = shot
            .geometry
            .to_physical(&rect)
            .context("the selection is off-screen")?;
        let path = capture::timestamped(&config.output_dir(), "png")?;
        shot.frame.crop(crop).save_png(&path)?;
        return Ok(Some(path));
    }
    let region = rect.to_pixels();
    capture_region(
        instance,
        config,
        mode,
        &region,
        Some(&shot.geometry.name),
        None,
    )
    .map(Some)
}

/// Captures `region` now: a fresh still, or a recording until stopped.
fn capture_region(
    instance: &Instance,
    config: &Config,
    mode: Mode,
    region: &PixelRect,
    output: Option<&str>,
    duration: Option<f64>,
) -> Result<PathBuf> {
    let dir = config.output_dir();
    if mode == Mode::Image {
        let path = capture::timestamped(&dir, "png")?;
        capture::grab_region(region, &path)?;
        return Ok(path);
    }
    let mp4 = capture::timestamped(&dir, "mp4")?;
    let recorder = record::Recorder::start(region, mp4)?;
    let stopped = match duration {
        Some(secs) => wait_or_toggle(instance, Duration::from_secs_f64(secs)),
        None => {
            let output = match output {
                Some(name) => name.to_owned(),
                None => output_at(&region.rect()).unwrap_or_default(),
            };
            ui::record_indicator(instance, &output, &region.rect(), mode.label())
        }
    };
    if let Err(error) = stopped {
        recorder.abort();
        return Err(error);
    }
    let mp4 = recorder.stop()?;
    if mode == Mode::Video {
        return Ok(mp4);
    }
    if config.notify {
        notify::message(
            "Converting to GIF",
            "This takes a moment for long recordings.",
        );
    }
    record::to_gif(&mp4, config)
}

/// Sleeps for `total`, ending early if another snapframe asks to stop.
fn wait_or_toggle(instance: &Instance, total: Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + total;
    while std::time::Instant::now() < deadline {
        if instance.drain().iter().any(|m| m == ipc::TOGGLE) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// The name of the Hyprland monitor holding most of `rect`, for the pill.
fn output_at(rect: &Rect) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Monitor {
        name: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        scale: f64,
    }
    let out = Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .ok()?;
    let monitors: Vec<Monitor> = serde_json::from_slice(&out.stdout).ok()?;
    monitors
        .into_iter()
        .filter_map(|m| {
            let bounds = Rect::new(m.x, m.y, m.width / m.scale, m.height / m.scale);
            let overlap = bounds.intersect(rect)?;
            Some((overlap.w * overlap.h, m.name))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, name)| name)
}

/// Prints the path, then serves the clipboard and the notification from
/// detached processes so this one can exit.
fn finish(config: &Config, path: &Path, copy: bool) -> Result<()> {
    println!("{}", path.display());
    if copy {
        spawn_detached(&["serve".as_ref(), path.as_os_str()])?;
    }
    if config.notify {
        let mut args = vec!["notify".as_ref(), path.as_os_str()];
        if copy {
            args.push("--copied".as_ref());
        }
        spawn_detached(&args)?;
    }
    Ok(())
}

fn spawn_detached(args: &[&std::ffi::OsStr]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("finding our own executable")?;
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("starting a helper process")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn region_mode_parses() {
        let cli = Cli::parse_from([
            "snapframe",
            "--mode",
            "gif",
            "--region",
            "1,2,3,4",
            "--duration",
            "2",
        ]);
        assert!(matches!(cli.mode, ModeArg::Gif));
        assert_eq!(cli.region.as_deref(), Some("1,2,3,4"));
    }

    #[test]
    fn bad_modes_fail() {
        assert!(Cli::try_parse_from(["snapframe", "--mode", "webm"]).is_err());
    }
}
