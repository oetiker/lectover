//! Ogg/Opus encoding (mono, 48 kbit/s), replacing Python's `ffmpeg -c:a libopus -b:a 48k -ac 1`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use ogg::writing::{PacketWriteEndInfo, PacketWriter};

use super::resample::resample;

pub const BITRATE: i32 = 48_000;
/// Ogg/Opus granule positions always count 48 kHz samples (RFC 7845).
const GRANULE_RATE: u64 = 48_000;
/// Logical stream serial; fixed so equal audio gives equal bytes (and an equal content hash).
const SERIAL: u32 = 0x4c45_4354; // "LECT"
const VENDOR: &str = "lectover";

fn opus_rate(sr: u32) -> Option<SampleRate> {
    Some(match sr {
        8000 => SampleRate::Hz8000,
        12000 => SampleRate::Hz12000,
        16000 => SampleRate::Hz16000,
        24000 => SampleRate::Hz24000,
        48000 => SampleRate::Hz48000,
        _ => return None,
    })
}

fn opus_head(pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1); // version
    h.push(1); // channel count: mono
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&input_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // channel mapping family 0 (mono/stereo)
    h
}

fn opus_tags() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(b"OpusTags");
    t.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    t.extend_from_slice(VENDOR.as_bytes());
    t.extend_from_slice(&0u32.to_le_bytes()); // no user comments
    t
}

/// Encode mono audio to Ogg/Opus (48 kbit/s) at `path`.
///
/// Rates Opus accepts natively (8/12/16/24/48 kHz) are encoded as is; others are resampled
/// to 48 kHz first. Frames are 20 ms; the pre-skip is the encoder lookahead.
pub fn encode_opus(audio: &[f32], sr: u32, path: &Path) -> Result<()> {
    let (audio, rate) = match opus_rate(sr) {
        Some(_) => (std::borrow::Cow::Borrowed(audio), sr),
        None => (std::borrow::Cow::Owned(resample(audio, sr, 48_000)), 48_000),
    };
    let opus_err = |e: audiopus::Error| anyhow!("opus encoder: {e}");
    let mut enc = Encoder::new(
        opus_rate(rate).expect("supported rate"),
        Channels::Mono,
        Application::Audio,
    )
    .map_err(opus_err)?;
    enc.set_bitrate(Bitrate::BitsPerSecond(BITRATE))
        .map_err(opus_err)?;
    let lookahead = enc.lookahead().map_err(opus_err)? as usize;
    let to_granule = |samples: usize| samples as u64 * GRANULE_RATE / rate as u64;
    let pre_skip = to_granule(lookahead);

    let frame = rate as usize / 50; // 20 ms
    // encode enough frames that the lookahead delay still flushes the last real sample
    let n_frames = (audio.len() + lookahead).div_ceil(frame).max(1);
    let end_granule = pre_skip + to_granule(audio.len());

    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut w = PacketWriter::new(BufWriter::new(file));
    w.write_packet(
        opus_head(pre_skip as u16, sr),
        SERIAL,
        PacketWriteEndInfo::EndPage,
        0,
    )?;
    w.write_packet(opus_tags(), SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    let mut pcm = vec![0.0f32; frame];
    let mut packet = vec![0u8; 4000];
    for k in 0..n_frames {
        let start = (k * frame).min(audio.len());
        let end = ((k + 1) * frame).min(audio.len());
        pcm.fill(0.0);
        pcm[..end - start].copy_from_slice(&audio[start..end]);
        let len = enc.encode_float(&pcm, &mut packet).map_err(opus_err)?;
        let last = k + 1 == n_frames;
        let granule = if last {
            end_granule
        } else {
            to_granule((k + 1) * frame)
        };
        let info = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        w.write_packet(packet[..len].to_vec(), SERIAL, info, granule)?;
    }
    w.into_inner()
        .flush()
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
