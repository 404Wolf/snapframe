//! Still captures: each output's frozen frame (what the overlay shows and
//! crops), and fresh region captures for delayed shots, via `grim`.

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

use crate::geom::PixelRect;

/// An opaque RGB frame of one output, in its physical pixels.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Row-major RGB, three bytes a pixel.
    pub rgb: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    /// Grabs `output` (by its Wayland name) as it is now.
    pub fn grab_output(output: &str) -> Result<Frame> {
        let out = Command::new("grim")
            .args(["-t", "ppm", "-o", output, "-"])
            .output()
            .context("running grim (is it installed?)")?;
        if !out.status.success() {
            bail!(
                "grim -o {output}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Frame::from_ppm(&out.stdout)
    }

    /// Parses a binary (P6, 8-bit) PPM, which is what grim writes.
    pub fn from_ppm(data: &[u8]) -> Result<Frame> {
        let mut fields = Vec::with_capacity(4);
        let mut i = 0;
        while fields.len() < 4 {
            while i < data.len() && data[i].is_ascii_whitespace() {
                i += 1;
            }
            if data.get(i) == Some(&b'#') {
                while i < data.len() && data[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            let start = i;
            while i < data.len() && !data[i].is_ascii_whitespace() {
                i += 1;
            }
            ensure!(i > start, "truncated PPM header");
            fields.push(std::str::from_utf8(&data[start..i])?);
        }
        ensure!(fields[0] == "P6", "not a binary PPM");
        let width: u32 = fields[1].parse()?;
        let height: u32 = fields[2].parse()?;
        ensure!(fields[3] == "255", "unsupported PPM depth {}", fields[3]);
        let pixels = &data[i + 1..];
        let len = width as usize * height as usize * 3;
        ensure!(pixels.len() >= len, "PPM is missing pixel data");
        Ok(Frame {
            width,
            height,
            rgb: pixels[..len].to_vec(),
        })
    }

    /// The `(x, y, w, h)` physical-pixel region of this frame.
    pub fn crop(&self, (x, y, w, h): (u32, u32, u32, u32)) -> Frame {
        let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
        for row in y..y + h {
            let start = (row as usize * self.width as usize + x as usize) * 3;
            rgb.extend_from_slice(&self.rgb[start..start + w as usize * 3]);
        }
        Frame {
            width: w,
            height: h,
            rgb,
        }
    }

    pub fn save_png(&self, path: &Path) -> Result<()> {
        let file =
            std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.write_header()?.write_image_data(&self.rgb)?;
        Ok(())
    }
}

/// Captures `region` (global logical pixels) straight to a PNG at `path`.
pub fn grab_region(region: &PixelRect, path: &Path) -> Result<()> {
    let out = Command::new("grim")
        .args(["-t", "png", "-g", &region.geometry()])
        .arg(path)
        .output()
        .context("running grim (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "grim -g {}: {}",
            region.geometry(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// A new file in `dir` named for the local time, e.g. `2026-10-24_18-03-07.png`.
pub fn timestamped(dir: &Path, extension: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = now.format(time::macros::format_description!(
        "[year]-[month]-[day]_[hour]-[minute]-[second]"
    ))?;
    let mut path = dir.join(format!("{stamp}.{extension}"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{stamp}-{n}.{extension}"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppm(w: u32, h: u32) -> Vec<u8> {
        let mut d = format!("P6\n# grim\n{w} {h}\n255\n").into_bytes();
        for i in 0..w * h {
            d.extend_from_slice(&[i as u8, (i * 2) as u8, (i * 3) as u8]);
        }
        d
    }

    #[test]
    fn parses_ppm_with_comments() {
        let f = Frame::from_ppm(&ppm(3, 2)).unwrap();
        assert_eq!((f.width, f.height, f.rgb.len()), (3, 2, 18));
        assert_eq!(&f.rgb[3..6], &[1, 2, 3]);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(Frame::from_ppm(b"P3\n1 1\n255\n0 0 0").is_err());
        assert!(Frame::from_ppm(b"P6\n2 2\n255\n\x00").is_err());
    }

    #[test]
    fn crops_rows_and_columns() {
        let f = Frame::from_ppm(&ppm(4, 3)).unwrap();
        let c = f.crop((1, 1, 2, 2));
        assert_eq!((c.width, c.height), (2, 2));
        // Pixel (1,1) is index 5.
        assert_eq!(&c.rgb[0..3], &[5, 10, 15]);
        // Pixel (2,2) is index 10.
        assert_eq!(&c.rgb[9..12], &[10, 20, 30]);
    }

    #[test]
    fn saves_a_readable_png_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let f = Frame::from_ppm(&ppm(4, 3)).unwrap();
        let a = timestamped(dir.path(), "png").unwrap();
        f.save_png(&a).unwrap();
        let b = timestamped(dir.path(), "png").unwrap();
        assert_ne!(a, b);
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&a).unwrap()));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (4, 3));
    }
}
