//! Settings from `$XDG_CONFIG_HOME/snapframe/config.toml`; every key is optional.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where captures are saved. `~` expands to the home directory.
    pub output_dir: PathBuf,
    /// Seconds the delay toggle waits before capturing or recording.
    pub delay_secs: u64,
    /// Frames per second of converted GIFs.
    pub gif_fps: u32,
    /// GIFs wider than this are scaled down to it.
    pub gif_max_width: u32,
    /// gifski quality, 1 to 100.
    pub gif_quality: u8,
    /// Whether to show a notification when a capture is saved.
    pub notify: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("~/Screenshots"),
            delay_secs: 3,
            gif_fps: 15,
            gif_max_width: 1200,
            gif_quality: 90,
            notify: true,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("snapframe/config.toml"))
    }

    pub fn load() -> Result<Config> {
        let Some(path) = Self::path().filter(|p| p.exists()) else {
            return Ok(Config::default());
        };
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        Ok(toml::from_str(text)?)
    }

    /// `output_dir` with `~` expanded.
    pub fn output_dir(&self) -> PathBuf {
        match self.output_dir.strip_prefix("~") {
            Ok(rest) => dirs::home_dir().unwrap_or_default().join(rest),
            Err(_) => self.output_dir.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_use_defaults() {
        let c = Config::parse("delay_secs = 5\n").unwrap();
        assert_eq!(c.delay_secs, 5);
        assert_eq!(c.gif_fps, 15);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("delay = 5\n").is_err());
    }

    #[test]
    fn tilde_expands_to_home() {
        let c = Config::default();
        assert_eq!(
            c.output_dir(),
            dirs::home_dir().unwrap().join("Screenshots")
        );
        let abs = Config {
            output_dir: "/srv/shots".into(),
            ..Config::default()
        };
        assert_eq!(abs.output_dir(), PathBuf::from("/srv/shots"));
    }
}
