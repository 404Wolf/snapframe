//! Screen recording via `wf-recorder`, and MP4 to GIF via `ffmpeg` piped into `gifski`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::geom::PixelRect;

pub struct Recorder {
    child: Child,
    path: PathBuf,
    log: PathBuf,
}

impl Recorder {
    /// Starts recording `region` (global logical pixels) to the MP4 at `path`.
    pub fn start(region: &PixelRect, path: PathBuf) -> Result<Recorder> {
        let log = path.with_extension("log");
        let stderr = std::fs::File::create(&log).context("creating the recorder log")?;
        let child = Command::new("wf-recorder")
            .args(["-y", "-x", "yuv420p", "-g", &region.geometry(), "-f"])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .context("running wf-recorder (is it installed?)")?;
        let mut recorder = Recorder { child, path, log };
        // A bad geometry or missing protocol fails at once; surface that now.
        std::thread::sleep(Duration::from_millis(300));
        if let Some(status) = recorder.child.try_wait()? {
            let reason = recorder.log_tail();
            let _ = std::fs::remove_file(&recorder.log);
            bail!("wf-recorder exited at once ({status}): {reason}");
        }
        Ok(recorder)
    }

    /// Stops cleanly (SIGINT lets it finish the file), killing it if it hangs.
    pub fn stop(mut self) -> Result<PathBuf> {
        let pid = self.child.id() as libc::pid_t;
        // SAFETY: signalling our own child process by its pid.
        unsafe { libc::kill(pid, libc::SIGINT) };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let ok = std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 0);
        let reason = self.log_tail();
        let _ = std::fs::remove_file(&self.log);
        if !ok {
            let _ = std::fs::remove_file(&self.path);
            bail!("the recording is empty: {reason}");
        }
        Ok(self.path)
    }

    /// Stops and discards the recording.
    pub fn abort(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.log);
    }

    fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(3)..].join(" | ")
    }
}

/// Converts the MP4 at `mp4` to a GIF beside it, removing the MP4 on success.
pub fn to_gif(mp4: &Path, config: &Config) -> Result<PathBuf> {
    let gif = mp4.with_extension("gif");
    let filter = format!(
        "fps={},scale=min({}\\,iw):-2:flags=lanczos",
        config.gif_fps, config.gif_max_width
    );
    let mut ffmpeg = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(mp4)
        .args([
            "-vf",
            &filter,
            "-f",
            "yuv4mpegpipe",
            "-pix_fmt",
            "yuv444p",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running ffmpeg (is it installed?)")?;
    let frames = ffmpeg.stdout.take().context("ffmpeg stdout")?;
    let gifski = Command::new("gifski")
        .args(["--quiet", "-Q", &config.gif_quality.to_string(), "-o"])
        .arg(&gif)
        .arg("-")
        .stdin(frames)
        .output()
        .context("running gifski (is it installed?)")?;
    let ffmpeg = ffmpeg.wait_with_output()?;
    if !ffmpeg.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&ffmpeg.stderr).trim());
    }
    if !gifski.status.success() {
        bail!("gifski: {}", String::from_utf8_lossy(&gifski.stderr).trim());
    }
    std::fs::remove_file(mp4).ok();
    Ok(gif)
}
