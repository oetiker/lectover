//! MP3 encoding (mono, 64 kbit/s CBR, MPEG-2 Layer III at 24 kHz) for browsers without Opus.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusty_mp3::{Error as Mp3Error, Mp3Encoder, Mp3EncoderConfig};

use super::resample::resample;

pub const MP3_KBPS: u32 = 64;
/// 24 kHz selects MPEG-2 Layer III, which has 576-sample frames (192 bytes at 64 kbit/s).
const MP3_RATE: u32 = 24_000;

/// Encode mono audio to MP3 (64 kbit/s CBR, 24 kHz) at `path`.
///
/// Other input rates are resampled to 24 kHz first. The first packet is the Info tag frame.
pub fn encode_mp3(audio: &[f32], sr: u32, path: &Path) -> Result<()> {
    let audio = if sr == MP3_RATE {
        std::borrow::Cow::Borrowed(audio)
    } else {
        std::borrow::Cow::Owned(resample(audio, sr, MP3_RATE))
    };
    let mut enc = Mp3Encoder::new(Mp3EncoderConfig {
        bitrate_kbps: MP3_KBPS,
        vbr_quality: None,
    });
    enc.push_pcm_f32(&audio, 1, MP3_RATE)
        .map_err(|e| anyhow!("mp3 encode: {e:?}"))?;
    enc.finish();

    let mut out =
        BufWriter::new(File::create(path).with_context(|| format!("create {}", path.display()))?);
    // After finish() the first packet is the completed Info tag.
    loop {
        match enc.next_packet() {
            Ok(packet) => out.write_all(&packet)?,
            Err(Mp3Error::Eof) => break,
            Err(e) => return Err(anyhow!("mp3 encode: {e:?}")),
        }
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_mp3::Mp3Decoder;

    const FRAME_BYTES: usize = 192;

    fn sine(sr: u32, secs: f32) -> Vec<f32> {
        (0..(sr as f32 * secs) as usize)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin())
            .collect()
    }

    fn encode_to_bytes(audio: &[f32], sr: u32) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.mp3");
        encode_mp3(audio, sr, &path).unwrap();
        std::fs::read(path).unwrap()
    }

    #[test]
    fn mp3_header_is_mpeg2_l3_24k_mono_64k() {
        let bytes = encode_to_bytes(&sine(24_000, 1.0), 24_000);
        let h = &bytes[FRAME_BYTES..FRAME_BYTES + 4];
        assert_eq!(h[0], 0xFF);
        assert_eq!(h[1] >> 5, 0b111, "sync");
        assert_eq!((h[1] >> 3) & 0b11, 0b10, "MPEG-2");
        assert_eq!((h[1] >> 1) & 0b11, 0b01, "layer III");
        assert_eq!(h[2] >> 4, 0b1000, "64 kbit/s (MPEG-2 L3 index 8)");
        assert_eq!((h[2] >> 2) & 0b11, 0b01, "24 kHz");
        assert_eq!(h[3] >> 6, 0b11, "mono");
        assert_eq!(bytes.len() % FRAME_BYTES, 0);
    }

    #[test]
    fn mp3_has_info_tag() {
        let bytes = encode_to_bytes(&sine(24_000, 1.0), 24_000);
        assert!(bytes[..FRAME_BYTES].windows(4).any(|w| w == b"Info"));
    }

    #[test]
    fn mp3_decodes_to_input_length() {
        let input = sine(24_000, 1.0);
        let bytes = encode_to_bytes(&input, 24_000);
        let mut dec = Mp3Decoder::new();
        dec.push(&bytes);
        dec.flush();
        let mut decoded = 0usize;
        while let Ok(f) = dec.next_frame() {
            decoded += f.samples.len() / f.channels as usize;
        }
        // Encoder delay + decoder delay (1057) at the front, up to two frames of padding at the end.
        assert!(
            decoded >= input.len(),
            "decoded {decoded} < input {}",
            input.len()
        );
        assert!(
            decoded <= input.len() + 1057 + 2 * 576,
            "decoded {decoded} too long"
        );
    }

    #[test]
    fn mp3_is_deterministic() {
        let input = sine(24_000, 0.5);
        assert_eq!(
            encode_to_bytes(&input, 24_000),
            encode_to_bytes(&input, 24_000)
        );
    }

    /// Writes the encoder-delay probe (`probe.ogg`, `probe.mp3`, `delay.html`) to
    /// `$LECTOVER_PROBE_DIR` (relative paths resolve against the workspace root), for
    /// `player/probe/delay.html` under `lectover dev serve`.
    #[test]
    #[ignore]
    fn write_delay_probe() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = root.join(std::env::var("LECTOVER_PROBE_DIR").expect("set LECTOVER_PROBE_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        // 3 s of silence with a 5 ms full-scale 1 kHz burst (cosine phase, so the first sample
        // is already at full scale) starting at exactly 1.000 s and 2.000 s.
        let rate = MP3_RATE as usize;
        let mut audio = vec![0f32; 3 * rate];
        for onset in [rate, 2 * rate] {
            for i in 0..rate / 200 {
                audio[onset + i] =
                    (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / MP3_RATE as f32).cos();
            }
        }
        crate::audio::encode_opus(&audio, MP3_RATE, &dir.join("probe.ogg")).unwrap();
        encode_mp3(&audio, MP3_RATE, &dir.join("probe.mp3")).unwrap();
        std::fs::copy(root.join("player/probe/delay.html"), dir.join("delay.html")).unwrap();
    }

    #[test]
    fn mp3_resamples_other_rates() {
        let bytes = encode_to_bytes(&sine(22_050, 1.0), 22_050);
        assert_eq!((bytes[FRAME_BYTES + 2] >> 2) & 0b11, 0b01, "24 kHz");
    }
}
