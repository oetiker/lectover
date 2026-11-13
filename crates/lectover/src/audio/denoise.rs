//! Speech cleanup interface and the helpers around it.

use std::io::Write;

use anyhow::{bail, Result};

/// Longest clip the cleaner accepts, in seconds.
pub const MAX_CLEAN_SECS: f64 = 90.0;

/// Speech cleanup (denoise / dereverberation) of mono audio.
pub trait Denoise {
    /// Clean `pcm` (mono, `sr` Hz). The result has the same length and lies within [-1, 1].
    fn denoise(&mut self, pcm: &[f32], sr: u32) -> Result<Vec<f32>>;
}

/// Refuse clips longer than [`MAX_CLEAN_SECS`].
pub fn check_clean_len(n_samples: usize, sr: u32) -> Result<()> {
    let secs = n_samples as f64 / sr.max(1) as f64;
    if n_samples as f64 > MAX_CLEAN_SECS * sr as f64 {
        bail!("clip is {secs:.1} s long; cleaning is limited to {MAX_CLEAN_SECS:.0} s");
    }
    Ok(())
}

/// Noise floor of `wav`: the 10th-percentile energy of its 10 ms frames, relative to the
/// loudest frame, in dB (negative; lower is cleaner).
pub fn noise_floor_db(wav: &[f32], sr: u32) -> f64 {
    let frame = ((sr as usize) / 100).max(1);
    let mut energies: Vec<f64> = wav
        .chunks_exact(frame)
        .map(|f| f.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / frame as f64)
        .collect();
    if energies.is_empty() {
        return 0.0;
    }
    energies.sort_by(|a, b| a.total_cmp(b));
    let loudest = energies[energies.len() - 1];
    let p10 = energies[energies.len() / 10];
    10.0 * ((p10 + 1e-18) / (loudest + 1e-18)).log10()
}

/// Render `fraction` (clamped to [0, 1]) as `[#####-----]  50%`, `width` cells wide.
pub fn progress_bar(fraction: f32, width: usize) -> String {
    let f = if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    };
    let filled = ((f * width as f32).round() as usize).min(width);
    format!(
        "[{}{}] {:3}%",
        "#".repeat(filled),
        "-".repeat(width - filled),
        (f * 100.0).round() as u32
    )
}

/// Redraws one progress line on `out` with `\r`, only when the whole percent changes, and
/// ends the line with `\n` at 100 %. Write errors are ignored: progress is cosmetic.
pub struct ProgressLine<W: Write> {
    out: W,
    prefix: String,
    last_pct: Option<u32>,
}

impl<W: Write> ProgressLine<W> {
    pub fn new(out: W, prefix: impl Into<String>) -> Self {
        ProgressLine {
            out,
            prefix: prefix.into(),
            last_pct: None,
        }
    }

    pub fn update(&mut self, fraction: f32) {
        let f = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        let pct = (f * 100.0).round() as u32;
        if self.last_pct == Some(pct) {
            return;
        }
        self.last_pct = Some(pct);
        let end = if pct >= 100 { "\n" } else { "" };
        let _ = write!(self.out, "\r{}{}{}", self.prefix, progress_bar(f, 20), end);
        let _ = self.out.flush();
    }
}

/// A progress hook drawing on stderr, or `None` when stderr is not a terminal.
pub fn stderr_progress_hook() -> Option<Box<dyn FnMut(f32) + Send>> {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return None;
    }
    let mut line = ProgressLine::new(std::io::stderr(), "  Removing background noise ");
    Some(Box::new(move |f| line.update(f)))
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn bar_renders_and_clamps() {
        assert_eq!(progress_bar(0.0, 10), "[----------]   0%");
        assert_eq!(progress_bar(0.5, 10), "[#####-----]  50%");
        assert_eq!(progress_bar(1.0, 10), "[##########] 100%");
        assert_eq!(progress_bar(-3.0, 10), "[----------]   0%");
        assert_eq!(progress_bar(7.0, 10), "[##########] 100%");
        assert_eq!(progress_bar(f32::NAN, 10), "[----------]   0%");
    }

    #[test]
    fn line_throttles_to_whole_percent_and_ends_with_newline() {
        let mut buf = Vec::new();
        {
            let mut l = ProgressLine::new(&mut buf, "> ");
            l.update(0.001);
            l.update(0.004); // same percent: no redraw
            l.update(0.5);
            l.update(1.0);
            l.update(1.0);
        }
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.matches('\r').count(), 3, "{s:?}");
        assert!(s.ends_with("100%\n"), "{s:?}");
    }
}
