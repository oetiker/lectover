//! Voice recording: level calibration, triggered recording with pre-roll,
//! mid-take and post-take review/re-record, and saving to `voices/`.

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::audio::denoise::Denoise;
use crate::cli::RecordArgs;
use crate::design::{check_target, clip_paths, keep_old, write_wav16, REF_TEXT};
use crate::voices::add_voice;

pub const SAMPLE_RATE: u32 = 24_000;
pub const TEST_TEXT: &str =
    "Eins, zwei, drei — dies ist ein kurzer Mikrofontest in normaler Sprechlautstärke.";
/// Target peak is ~ -6 dBFS: 10^(-6/20) = ~0.501187.
pub const TARGET_PEAK: f32 = 0.501187;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserKey {
    Enter,
    Rerecord,
    Play,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordRequest {
    pub id: String,
    pub label: String,
    pub text: String,
    pub seconds: u32,
    pub force: bool,
}

pub trait AudioSession {
    fn device_name(&self) -> &str;
    fn sample_rate(&self) -> u32;
    /// Sample rate the input hardware itself runs at, when it differs from
    /// the rate of the delivered chunks (the OS may resample on the way).
    fn native_sample_rate(&self) -> u32 {
        self.sample_rate()
    }
    fn get_gain(&mut self) -> Option<f32>;
    fn set_gain(&mut self, gain: f32) -> Result<()>;
    fn next_chunk(&mut self) -> Result<Vec<f32>>;
    /// Discard queued input, including transients such as key clicks.
    fn flush_input(&mut self);
    fn poll_key(&mut self) -> Option<UserKey>;
    fn read_key(&mut self) -> Result<UserKey>;
    fn play(&mut self, pcm: &[f32], sr: u32) -> Result<()>;
    /// Terminal width in columns, used to keep `\r` status lines from wrapping.
    fn term_width(&self) -> usize {
        80
    }
}

/// Calibration gives up after this many 100 ms chunks (12 s) of test speech.
const CALIBRATION_MAX_CHUNKS: usize = 120;
/// Calibration runs at least this many chunks (3 s): the test sentence is longer.
const CALIBRATION_MIN_CHUNKS: usize = 30;
/// A pause of this many chunks (1.5 s) ends the calibration or the take;
/// pauses between words and sentences are shorter.
const END_PAUSE_CHUNKS: usize = 15;
/// A take ends at a pause only after this many chunks (2 s).
const TAKE_MIN_CHUNKS: usize = 20;
/// Step 2 arms its onset trigger only after this many quiet chunks (0.5 s).
const ARM_QUIET_CHUNKS: usize = 5;

/// Below this peak a chunk counts as a pause: halfway (in dB) between the
/// noise floor and the speech peak heard so far, so soft syllables on a
/// quiet microphone still count as speech.
pub fn pause_level(noise_peak: f32, speech_peak: f32) -> f32 {
    (noise_peak * speech_peak).sqrt().max(noise_peak * 2.0)
}

/// Onset threshold for speech detection, relative to the measured noise peak.
pub fn onset_threshold(noise_peak: f32) -> f32 {
    (noise_peak * 3.5).max(0.04)
}

/// Approximate display width of a char (emoji and CJK are double width).
fn char_width(c: char) -> usize {
    let u = c as u32;
    if u >= 0x1F300 || (0x2E80..=0xA4CF).contains(&u) || (0xAC00..=0xD7A3).contains(&u) {
        2
    } else {
        1
    }
}

/// Cut a status line so it fits in `width - 1` columns: a line that wraps
/// breaks `\r`-based in-place updates and scrolls the terminal.
pub fn fit_status(text: &str, width: usize) -> String {
    let max = width.saturating_sub(1).max(10);
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        let w = char_width(c);
        if used + w > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// Overwrite the current terminal line with a status message.
fn status(log: &mut dyn Write, width: usize, text: &str) -> Result<()> {
    write!(log, "\r{}\x1b[K", fit_status(text, width))?;
    log.flush()?;
    Ok(())
}

/// Estimate recording duration in seconds from word count (~2 words/sec, minimum 4s).
pub fn default_seconds(text: &str) -> u32 {
    let words = text.split_whitespace().count() as u32;
    (words / 2).max(4)
}

/// Audio kept from before the onset (three 100 ms chunks) and prepended to the take.
pub const PREROLL_SECS: f64 = 0.3;

/// Refuse a planned take (`seconds` plus pre-roll) that the cleaner would reject afterwards.
pub fn check_take_len(seconds: u32) -> Result<()> {
    let total = seconds as f64 + PREROLL_SECS;
    if total > crate::audio::denoise::MAX_CLEAN_SECS {
        bail!(
            "planned take of {seconds} s (+{PREROLL_SECS} s pre-roll) exceeds the {} s cleaning limit; use a shorter text or --seconds",
            crate::audio::denoise::MAX_CLEAN_SECS
        );
    }
    Ok(())
}

/// Format a peak amplitude (0.0 to 1.0) into a 10-char bar [████░░░░░░] and dBFS value.
pub fn vu_bar(peak: f32) -> (String, f32) {
    let db = if peak > 1e-5 {
        20.0 * peak.log10()
    } else {
        -60.0
    };
    let clamped_db = db.clamp(-48.0, 0.0);
    let ratio = (clamped_db + 48.0) / 48.0;
    let filled = (ratio * 10.0).round() as usize;
    let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(10 - filled));
    (bar, db)
}

/// Calculate the next hardware and software gain given the measured peak amplitude.
pub fn next_gain(hw_gain: Option<f32>, sw_gain: f32, peak: f32) -> (Option<f32>, f32) {
    if peak <= 1e-4 {
        return (hw_gain, sw_gain);
    }
    let target_ratio = TARGET_PEAK / peak;
    if let Some(hw) = hw_gain {
        let ideal_hw = hw * target_ratio;
        let new_hw = ideal_hw.clamp(0.05, 1.0);
        let remainder = ideal_hw / new_hw;
        let new_sw = (sw_gain * remainder).max(0.1);
        (Some(new_hw), new_sw)
    } else {
        let new_sw = (sw_gain * target_ratio).max(0.1);
        (None, new_sw)
    }
}

/// Warn when the microphone looks like a Bluetooth headset: its narrowband
/// (HFP) audio with built-in voice processing cleans poorly.
pub fn headset_mic_warning(sample_rate: u32, device_name: Option<&str>) -> Option<String> {
    const HINTS: [&str; 5] = ["airpods", "hands-free", "handsfree", "headset", "bluetooth"];
    let name = device_name.filter(|n| !n.trim().is_empty());
    let hinted = name.is_some_and(|n| {
        let lower = n.to_lowercase();
        HINTS.iter().any(|h| lower.contains(h))
    });
    if sample_rate > 16_000 && !hinted {
        return None;
    }
    let who = name.map(|n| format!(" \"{n}\"")).unwrap_or_default();
    Some(format!(
        "  Warning: the microphone{who} runs at {sample_rate} Hz and looks like a Bluetooth headset; \
its audio cleans poorly. Use the built-in or a wired microphone."
    ))
}

/// Record a voice clip with interactive calibration, countdown, and review.
pub fn record_voice(
    session: &mut dyn AudioSession,
    denoiser: &mut dyn Denoise,
    voices_dir: &Path,
    req: &RecordRequest,
    log: &mut dyn Write,
) -> Result<PathBuf> {
    check_target(voices_dir, &req.id, req.force)?;

    let initial_gain_str = match session.get_gain() {
        Some(g) => format!("{:.0}%", g * 100.0),
        None => "fixed".to_string(),
    };
    writeln!(
        log,
        "Microphone: {} (input gain: {initial_gain_str})",
        session.device_name()
    )?;

    if let Some(w) = headset_mic_warning(session.native_sample_rate(), Some(session.device_name()))
    {
        writeln!(log, "{w}")?;
    }

    // Step 1: Microphone level check
    writeln!(log, "Step 1/2 — Microphone calibration")?;
    write!(
        log,
        "  Measuring background noise (stay quiet for a moment)... "
    )?;
    log.flush()?;

    // Drop the start-up transient and the click of the key that launched us.
    session.flush_input();
    let mut raw_noise = 0.0f32;
    for _ in 0..5 {
        if let Some(UserKey::Quit) = session.poll_key() {
            bail!("recording cancelled by user");
        }
        let chunk = session.next_chunk()?;
        for &s in &chunk {
            raw_noise = raw_noise.max(s.abs());
        }
    }
    let mut noise_peak = raw_noise.max(0.001);
    let noise_db = 20.0 * noise_peak.max(1e-5).log10();
    writeln!(log, "done ({noise_db:.0} dBFS).")?;
    if raw_noise == 0.0 {
        writeln!(
            log,
            "  Warning: the microphone delivers pure digital silence. Check that your terminal app \
             may use the microphone (System Settings → Privacy & Security → Microphone)."
        )?;
    }

    writeln!(
        log,
        "  Please read this test sentence aloud at normal speaking volume:\n"
    )?;
    writeln!(log, "    \"{TEST_TEXT}\"\n")?;
    log.flush()?;

    // Wait for voice onset
    let onset_thresh = onset_threshold(noise_peak);
    loop {
        if let Some(UserKey::Quit) = session.poll_key() {
            bail!("recording cancelled by user");
        }
        let chunk = session.next_chunk()?;
        let p = chunk.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let (bar, db) = vu_bar(p);
        status(
            log,
            session.term_width(),
            &format!("  Level: {bar} {db:>3.0} dBFS  Waiting for voice... (Ctrl-C to cancel)"),
        )?;
        if p >= onset_thresh {
            break;
        }
    }

    // Measure the whole test sentence: it ends once the speaker has paused.
    // The meter below overwrites the "waiting" line in place.
    let mut speech_peak = 0.0f32;
    let mut quiet = 0usize;
    for i in 0..CALIBRATION_MAX_CHUNKS {
        if let Some(UserKey::Quit) = session.poll_key() {
            bail!("recording cancelled by user");
        }
        let chunk = session.next_chunk()?;
        let p = chunk.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        speech_peak = speech_peak.max(p);
        quiet = if p < pause_level(noise_peak, speech_peak) {
            quiet + 1
        } else {
            0
        };
        let (bar, db) = vu_bar(p);
        let elapsed_s = (i + 1) as f32 * 0.1;
        status(
            log,
            session.term_width(),
            &format!("  Calibrating: {bar} {db:>3.0} dBFS {elapsed_s:>4.1}s  read to the end, then pause"),
        )?;
        if i + 1 >= CALIBRATION_MIN_CHUNKS && quiet >= END_PAUSE_CHUNKS {
            break;
        }
    }
    writeln!(log)?;

    let old_hw = session.get_gain();
    let (next_hw, mut sw_gain) = next_gain(old_hw, 1.0, speech_peak);
    let mut applied_hw = None;
    if let (Some(old), Some(hw)) = (old_hw, next_hw) {
        match session.set_gain(hw) {
            Ok(()) => {
                applied_hw = Some(hw);
                // The noise floor scales with the hardware gain; keep the
                // onset threshold relative to the noise the take will see.
                if old > 0.0 {
                    noise_peak *= hw / old;
                }
            }
            Err(e) => {
                writeln!(
                    log,
                    "  Note: cannot set the input gain ({e:#}); using software gain instead."
                )?;
                sw_gain = next_gain(None, 1.0, speech_peak).1;
            }
        }
    }
    let onset_thresh = onset_threshold(noise_peak);

    let final_gain_str = match applied_hw {
        Some(hw) if (sw_gain - 1.0).abs() > 0.05 => {
            format!("{:.0}% + {:.1}x software", hw * 100.0, sw_gain)
        }
        Some(hw) => format!("{:.0}%", hw * 100.0),
        None => format!("{:.1}x software", sw_gain),
    };
    writeln!(
        log,
        "  Level check complete: input gain set to {final_gain_str} (target peak -6 dBFS).\n"
    )?;

    // Step 2: Voice recording
    let (wav, txt) = clip_paths(voices_dir, &req.id);
    let raw_wav = voices_dir.join(format!("{}.raw.wav", req.id));
    let final_trimmed: Vec<f32>;
    let final_raw: Vec<f32>;

    'take_loop: loop {
        writeln!(log, "Step 2/2 — Voice recording (up to {}s)", req.seconds)?;
        writeln!(
            log,
            "  Read the following text aloud. Recording starts as you speak and ends when you pause:\n"
        )?;
        writeln!(log, "    \"{}\"\n", req.text)?;
        log.flush()?;

        session.flush_input();

        // Keep ~200ms pre-roll while waiting for onset. The trigger arms only
        // after a short quiet stretch, so a speaker still reading the test
        // sentence does not start the take.
        let mut preroll = VecDeque::new();
        let mut quiet = 0usize;
        loop {
            if let Some(key) = session.poll_key() {
                if key == UserKey::Quit {
                    bail!("recording cancelled by user");
                }
                if key == UserKey::Rerecord {
                    continue 'take_loop;
                }
            }
            let chunk = session.next_chunk()?;
            let p = chunk.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            let armed = quiet >= ARM_QUIET_CHUNKS;
            let (bar, db) = vu_bar(p);
            let what = if armed {
                "Waiting for voice..."
            } else {
                "Waiting for quiet..."
            };
            status(
                log,
                session.term_width(),
                &format!("  Level: {bar} {db:>3.0} dBFS  {what} (Ctrl-C to cancel)"),
            )?;

            // Two chunks before onset plus the onset chunk: ~200 ms pre-roll.
            preroll.push_back(chunk);
            if preroll.len() > 3 {
                preroll.pop_front();
            }
            if p >= onset_thresh {
                if armed {
                    break;
                }
                quiet = 0;
            } else {
                quiet += 1;
            }
        }

        // The REC meter below overwrites the "waiting" line in place.
        let total_chunks = (req.seconds * 10) as usize;
        let mut recorded = Vec::new();
        let mut take_peak = 0.0f32;
        for chunk in preroll {
            take_peak = chunk.iter().fold(take_peak, |m, &s| m.max(s.abs()));
            recorded.extend(chunk);
        }
        let mut quiet = 0usize;

        let mut aborted = false;
        for i in 0..total_chunks {
            if let Some(key) = session.poll_key() {
                if key == UserKey::Quit {
                    bail!("recording cancelled by user");
                }
                if key == UserKey::Rerecord {
                    aborted = true;
                    break;
                }
            }
            let chunk = session.next_chunk()?;
            let p = chunk.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            recorded.extend(chunk);
            take_peak = take_peak.max(p);
            quiet = if p < pause_level(noise_peak, take_peak) {
                quiet + 1
            } else {
                0
            };

            let (bar, db) = vu_bar(p);
            let secs = (i + 1) / 10;
            status(
                log,
                session.term_width(),
                &format!(
                    "  🔴 REC {:02}:{:02} {bar} {db:>3.0} dBFS  pause to end | [r] redo | Ctrl-C",
                    secs / 60,
                    secs % 60
                ),
            )?;
            if i + 1 >= TAKE_MIN_CHUNKS && quiet >= END_PAUSE_CHUNKS {
                break;
            }
        }
        writeln!(log)?;

        if aborted {
            writeln!(log, "  Take discarded. Re-starting...\n")?;
            continue 'take_loop;
        }

        let amplified: Vec<f32> = recorded.into_iter().map(|s| s * sw_gain).collect();
        let pcm24k = if session.sample_rate() != SAMPLE_RATE {
            crate::audio::resample(&amplified, session.sample_rate(), SAMPLE_RATE)
        } else {
            amplified
        };

        writeln!(log, "  Removing background noise...")?;
        let cleaned = denoiser.denoise(&pcm24k, SAMPLE_RATE)?;
        writeln!(log, "  Trimming silence and leveling audio...")?;
        let (trimmed, trim) = crate::audio::trim_and_level_with(&cleaned, SAMPLE_RATE);
        if trimmed.is_empty() {
            writeln!(log, "  Audio was empty after trimming. Re-starting...\n")?;
            continue 'take_loop;
        }

        let dur = trimmed.len() as f64 / SAMPLE_RATE as f64;
        writeln!(log, "Take ready ({dur:.1}s). Playing preview...\n")?;
        log.flush()?;
        session.play(&trimmed, SAMPLE_RATE)?;

        loop {
            writeln!(
                log,
                "  [Enter] save to {}  |  [r] re-record  |  [p] play preview  |  [q] quit",
                wav.display()
            )?;
            write!(log, "> ")?;
            log.flush()?;
            let key = session.read_key()?;
            // Keys are not echoed in raw mode; end the "> " prompt line.
            writeln!(log)?;
            match key {
                UserKey::Play => {
                    writeln!(log, "Playing preview...")?;
                    session.play(&trimmed, SAMPLE_RATE)?;
                }
                UserKey::Rerecord => {
                    writeln!(log, "\nRe-starting recording...\n")?;
                    continue 'take_loop;
                }
                UserKey::Quit => {
                    bail!("recording cancelled by user");
                }
                UserKey::Enter => {
                    final_raw = crate::audio::apply_trim(&pcm24k, SAMPLE_RATE, &trim);
                    final_trimmed = trimmed;
                    break 'take_loop;
                }
            }
        }
    }

    fs::create_dir_all(voices_dir).with_context(|| format!("creating {}", voices_dir.display()))?;
    keep_old(&[&wav, &raw_wav, &txt], log)?;
    write_wav16(&wav, &final_trimmed, SAMPLE_RATE)?;
    write_wav16(&raw_wav, &final_raw, SAMPLE_RATE)?;
    fs::write(&txt, format!("{}\n", req.text))
        .with_context(|| format!("writing {}", txt.display()))?;
    add_voice(voices_dir, &req.id, &req.label)?;
    let dur = final_trimmed.len() as f64 / SAMPLE_RATE as f64;
    writeln!(
        log,
        "recorded voice {} ({:.1}s) -> {}",
        req.id,
        dur,
        wav.display()
    )?;
    Ok(wav)
}

/// Execute `lectover voice record`.
pub fn run(args: &RecordArgs, settings: &crate::settings::Settings) -> Result<()> {
    let voices_dir = &settings.voices;
    let text = args.text.clone().unwrap_or_else(|| REF_TEXT.to_string());
    // The take ends when the speaker pauses; this is only the upper bound,
    // generous for slow readers.
    let seconds = args.seconds.unwrap_or_else(|| default_seconds(&text) * 2);
    check_take_len(seconds)?;
    let req = RecordRequest {
        id: args.id.clone(),
        label: args.label.clone(),
        text,
        seconds,
        force: args.force,
    };
    check_target(voices_dir, &req.id, req.force)?;
    // Load the cleaner first: a missing model must fail before the microphone opens.
    let gguf = crate::models::ensure_reuse_gguf(
        &settings.models,
        &crate::ffi::convert_reuse,
        &mut std::io::stdout(),
    )?;
    let mut denoiser = crate::ffi::ReuseDenoiser::load(
        &gguf,
        crate::ffi::resolve_device(args.device.or(settings.device))?,
    )?;
    denoiser.set_progress(crate::audio::denoise::stderr_progress_hook());
    let mut session = create_session()?;
    record_voice(
        &mut *session,
        &mut denoiser,
        voices_dir,
        &req,
        &mut std::io::stdout(),
    )?;
    Ok(())
}

// --- macOS CoreAudio & Terminal Implementation ---

#[cfg(target_os = "macos")]
pub fn create_session() -> Result<Box<dyn AudioSession>> {
    MacAudioSession::new()
}

#[cfg(not(target_os = "macos"))]
pub fn create_session() -> Result<Box<dyn AudioSession>> {
    bail!("microphone recording is currently only implemented for macOS");
}

#[cfg(target_os = "macos")]
#[allow(non_snake_case, non_upper_case_globals, dead_code)]
mod macos {
    use super::*;
    use std::ffi::{c_void, CStr};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::Duration;

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct AudioObjectPropertyAddress {
        mSelector: u32,
        mScope: u32,
        mElement: u32,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct AudioStreamBasicDescription {
        mSampleRate: f64,
        mFormatID: u32,
        mFormatFlags: u32,
        mBytesPerPacket: u32,
        mFramesPerPacket: u32,
        mBytesPerFrame: u32,
        mChannelsPerFrame: u32,
        mBitsPerChannel: u32,
        mReserved: u32,
    }

    #[repr(C)]
    struct AudioQueueBuffer {
        mAudioDataBytesCapacity: u32,
        mAudioData: *mut c_void,
        mAudioDataByteSize: u32,
        mUserData: *mut c_void,
        mPacketDescriptionCapacity: u32,
        mPacketDescriptions: *mut c_void,
        mPacketDescriptionCount: u32,
    }

    type AudioQueueRef = *mut c_void;
    type AudioQueueBufferRef = *mut AudioQueueBuffer;
    type AudioDeviceID = u32;

    const kAudioObjectSystemObject: u32 = 1;
    const kAudioHardwarePropertyDefaultInputDevice: u32 = 0x64496e20; // 'dIn '
    const kAudioObjectPropertyScopeGlobal: u32 = 0x676c6f62; // 'glob'
    const kAudioObjectPropertyScopeInput: u32 = 0x696e7074; // 'inpt'
    const kAudioObjectPropertyElementMain: u32 = 0;
    const kAudioDevicePropertyDeviceName: u32 = 0x6e616d65; // 'name'
    const kAudioDevicePropertyNominalSampleRate: u32 = 0x6e737274; // 'nsrt'
    const kAudioDevicePropertyVolumeScalar: u32 = 0x766f6c6d; // 'volm'

    const kAudioFormatLinearPCM: u32 = 0x6c70636d; // 'lpcm'
    const kAudioFormatFlagIsFloat: u32 = 1;
    const kAudioFormatFlagIsPacked: u32 = 8;

    extern "C" {
        fn AudioObjectGetPropertyData(
            inObjectID: u32,
            inAddress: *const AudioObjectPropertyAddress,
            inQualifierDataSize: u32,
            inQualifierData: *const c_void,
            ioDataSize: *mut u32,
            outData: *mut c_void,
        ) -> i32;

        fn AudioObjectSetPropertyData(
            inObjectID: u32,
            inAddress: *const AudioObjectPropertyAddress,
            inQualifierDataSize: u32,
            inQualifierData: *const c_void,
            inDataSize: u32,
            inData: *const c_void,
        ) -> i32;

        fn AudioQueueNewInput(
            inFormat: *const AudioStreamBasicDescription,
            inCallbackProc: extern "C" fn(
                *mut c_void,
                AudioQueueRef,
                AudioQueueBufferRef,
                *const c_void,
                u32,
                *const c_void,
            ),
            inUserData: *mut c_void,
            inCallbackRunLoop: *mut c_void,
            inCallbackRunLoopMode: *mut c_void,
            inFlags: u32,
            outAQ: *mut AudioQueueRef,
        ) -> i32;

        fn AudioQueueAllocateBuffer(
            inAQ: AudioQueueRef,
            inBufferByteSize: u32,
            outBuffer: *mut AudioQueueBufferRef,
        ) -> i32;
        fn AudioQueueEnqueueBuffer(
            inAQ: AudioQueueRef,
            inBuffer: AudioQueueBufferRef,
            inNumPacketDescs: u32,
            inPacketDescs: *const c_void,
        ) -> i32;
        fn AudioQueueStart(inAQ: AudioQueueRef, inStartTime: *const c_void) -> i32;
        fn AudioQueueStop(inAQ: AudioQueueRef, inImmediate: bool) -> i32;
        fn AudioQueueDispose(inAQ: AudioQueueRef, inImmediate: bool) -> i32;
    }

    static HAS_ORIG_TERMIOS: AtomicBool = AtomicBool::new(false);
    static mut GLOBAL_ORIG_TERMIOS: libc::termios = unsafe { std::mem::zeroed() };

    // Original input gain of the device, restored on exit (also on Ctrl-C):
    // the volume scalar is a system-wide setting, not per process.
    static HAS_ORIG_GAIN: AtomicBool = AtomicBool::new(false);
    static ORIG_GAIN_DEVICE: AtomicU32 = AtomicU32::new(0);
    static ORIG_GAIN_BITS: AtomicU32 = AtomicU32::new(0);

    const GAIN_ADDR: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyVolumeScalar,
        mScope: kAudioObjectPropertyScopeInput,
        mElement: 1,
    };

    fn write_gain(device: AudioDeviceID, gain: f32) -> i32 {
        unsafe {
            AudioObjectSetPropertyData(
                device,
                &GAIN_ADDR,
                0,
                std::ptr::null(),
                std::mem::size_of::<f32>() as u32,
                &gain as *const _ as *const c_void,
            )
        }
    }

    fn restore_orig_gain() {
        if HAS_ORIG_GAIN.swap(false, Ordering::SeqCst) {
            let device = ORIG_GAIN_DEVICE.load(Ordering::SeqCst);
            let gain = f32::from_bits(ORIG_GAIN_BITS.load(Ordering::SeqCst));
            write_gain(device, gain);
        }
    }

    extern "C" fn sigint_handler(_sig: libc::c_int) {
        if HAS_ORIG_TERMIOS.load(Ordering::SeqCst) {
            unsafe {
                libc::tcsetattr(
                    libc::STDIN_FILENO,
                    libc::TCSANOW,
                    std::ptr::addr_of!(GLOBAL_ORIG_TERMIOS),
                );
            }
        }
        // Not strictly async-signal-safe, but we _exit right after and the
        // alternative is leaving the system input level changed for good.
        restore_orig_gain();
        unsafe {
            let msg = b"\r\n\nRecording cancelled.\r\n";
            libc::write(libc::STDERR_FILENO, msg.as_ptr() as *const _, msg.len());
            libc::_exit(130);
        }
    }

    struct RawTerminal {
        orig: libc::termios,
    }

    impl RawTerminal {
        fn enter() -> Result<Self> {
            unsafe {
                if libc::isatty(libc::STDIN_FILENO) == 0 {
                    bail!("voice record needs an interactive terminal (stdin is not a tty)");
                }
                let mut orig = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut orig) != 0 {
                    bail!("cannot get terminal attributes");
                }
                GLOBAL_ORIG_TERMIOS = orig;
                HAS_ORIG_TERMIOS.store(true, Ordering::SeqCst);

                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = sigint_handler as *const () as usize;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
                libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());

                let mut raw = orig;
                raw.c_lflag &= !(libc::ECHO | libc::ICANON);
                // Keep ISIG enabled so Ctrl-C raises SIGINT
                raw.c_lflag |= libc::ISIG;
                // Preserve output processing so \n -> \r\n prevents staircase effect
                raw.c_oflag |= libc::OPOST | libc::ONLCR;
                // VMIN=0/VTIME=0 already makes read() return immediately. Do NOT
                // set O_NONBLOCK: the tty's file description is shared with
                // stdout and with the parent shell.
                raw.c_cc[libc::VMIN] = 0;
                raw.c_cc[libc::VTIME] = 0;

                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                    bail!("cannot set raw terminal mode");
                }
                Ok(Self { orig })
            }
        }
    }

    impl Drop for RawTerminal {
        fn drop(&mut self) {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig);
                HAS_ORIG_TERMIOS.store(false, Ordering::SeqCst);

                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = libc::SIG_DFL;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
                libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
            }
        }
    }

    struct QueueContext {
        sender: Sender<Vec<f32>>,
    }

    extern "C" fn audio_input_cb(
        user_data: *mut c_void,
        aq: AudioQueueRef,
        buf: AudioQueueBufferRef,
        _start_time: *const c_void,
        _num_packets: u32,
        _packet_descs: *const c_void,
    ) {
        unsafe {
            let ctx = user_data as *const QueueContext;
            let samples = (*buf).mAudioData as *const f32;
            let n = (*buf).mAudioDataByteSize as usize / 4;
            if n > 0 {
                let slice = std::slice::from_raw_parts(samples, n);
                (*ctx).sender.send(slice.to_vec()).ok();
            }
            AudioQueueEnqueueBuffer(aq, buf, 0, std::ptr::null());
        }
    }

    pub struct MacAudioSession {
        device_id: AudioDeviceID,
        device_name: String,
        queue: AudioQueueRef,
        _ctx: Box<QueueContext>,
        rx: Receiver<Vec<f32>>,
        _term: RawTerminal,
    }

    impl MacAudioSession {
        pub fn new() -> Result<Box<dyn AudioSession>> {
            // Fail before touching the audio device if there is no tty.
            let term = RawTerminal::enter()?;

            let mut device_id: AudioDeviceID = 0;
            let addr = AudioObjectPropertyAddress {
                mSelector: kAudioHardwarePropertyDefaultInputDevice,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            };
            let mut size = std::mem::size_of::<AudioDeviceID>() as u32;
            let status = unsafe {
                AudioObjectGetPropertyData(
                    kAudioObjectSystemObject,
                    &addr,
                    0,
                    std::ptr::null(),
                    &mut size,
                    &mut device_id as *mut _ as *mut c_void,
                )
            };
            if status != 0 || device_id == 0 {
                bail!("no default input device found (status {status})");
            }

            let mut name_buf = [0u8; 256];
            let name_addr = AudioObjectPropertyAddress {
                mSelector: kAudioDevicePropertyDeviceName,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            };
            let mut name_size = name_buf.len() as u32;
            let name_status = unsafe {
                AudioObjectGetPropertyData(
                    device_id,
                    &name_addr,
                    0,
                    std::ptr::null(),
                    &mut name_size,
                    name_buf.as_mut_ptr() as *mut c_void,
                )
            };
            let device_name = if name_status == 0 && name_size > 0 {
                CStr::from_bytes_until_nul(&name_buf)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "Default Microphone".to_string())
            } else {
                "Default Microphone".to_string()
            };

            let asbd = AudioStreamBasicDescription {
                mSampleRate: SAMPLE_RATE as f64,
                mFormatID: kAudioFormatLinearPCM,
                mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
                mBytesPerPacket: 4,
                mFramesPerPacket: 1,
                mBytesPerFrame: 4,
                mChannelsPerFrame: 1,
                mBitsPerChannel: 32,
                mReserved: 0,
            };

            let (tx, rx) = channel();
            let ctx = Box::new(QueueContext { sender: tx });
            let mut queue: AudioQueueRef = std::ptr::null_mut();

            let status = unsafe {
                AudioQueueNewInput(
                    &asbd,
                    audio_input_cb,
                    &*ctx as *const _ as *mut c_void,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                    &mut queue,
                )
            };
            if status != 0 {
                bail!("AudioQueueNewInput failed with status {status}");
            }

            // From here on, Drop disposes the queue on every error path.
            let mut session = Self {
                device_id,
                device_name,
                queue,
                _ctx: ctx,
                rx,
                _term: term,
            };

            // 3 buffers of 100ms each (2400 samples * 4 = 9600 bytes)
            let buffer_bytes = (SAMPLE_RATE / 10) * 4;
            for _ in 0..3 {
                let mut buf: AudioQueueBufferRef = std::ptr::null_mut();
                let status = unsafe { AudioQueueAllocateBuffer(queue, buffer_bytes, &mut buf) };
                if status != 0 {
                    bail!("AudioQueueAllocateBuffer failed with status {status}");
                }
                let status = unsafe { AudioQueueEnqueueBuffer(queue, buf, 0, std::ptr::null()) };
                if status != 0 {
                    bail!("AudioQueueEnqueueBuffer failed with status {status}");
                }
            }

            let status = unsafe { AudioQueueStart(queue, std::ptr::null()) };
            if status != 0 {
                bail!(
                    "AudioQueueStart failed with status {status} \
                     (check System Settings → Privacy & Security → Microphone)"
                );
            }

            if let Some(g) = session.get_gain() {
                ORIG_GAIN_DEVICE.store(session.device_id, Ordering::SeqCst);
                ORIG_GAIN_BITS.store(g.to_bits(), Ordering::SeqCst);
                HAS_ORIG_GAIN.store(true, Ordering::SeqCst);
            }
            Ok(Box::new(session))
        }
    }

    impl Drop for MacAudioSession {
        fn drop(&mut self) {
            unsafe {
                AudioQueueStop(self.queue, true);
                AudioQueueDispose(self.queue, true);
            }
            restore_orig_gain();
        }
    }

    impl AudioSession for MacAudioSession {
        fn device_name(&self) -> &str {
            &self.device_name
        }

        fn sample_rate(&self) -> u32 {
            SAMPLE_RATE
        }

        fn native_sample_rate(&self) -> u32 {
            let addr = AudioObjectPropertyAddress {
                mSelector: kAudioDevicePropertyNominalSampleRate,
                mScope: kAudioObjectPropertyScopeInput,
                mElement: kAudioObjectPropertyElementMain,
            };
            let mut rate: f64 = 0.0;
            let mut size = std::mem::size_of::<f64>() as u32;
            let status = unsafe {
                AudioObjectGetPropertyData(
                    self.device_id,
                    &addr,
                    0,
                    std::ptr::null(),
                    &mut size,
                    &mut rate as *mut _ as *mut c_void,
                )
            };
            if status == 0 && rate >= 1.0 {
                rate as u32
            } else {
                SAMPLE_RATE
            }
        }

        fn get_gain(&mut self) -> Option<f32> {
            let mut vol: f32 = 0.0;
            let mut size = std::mem::size_of::<f32>() as u32;
            let status = unsafe {
                AudioObjectGetPropertyData(
                    self.device_id,
                    &GAIN_ADDR,
                    0,
                    std::ptr::null(),
                    &mut size,
                    &mut vol as *mut _ as *mut c_void,
                )
            };
            if status == 0 {
                Some(vol)
            } else {
                None
            }
        }

        fn set_gain(&mut self, gain: f32) -> Result<()> {
            let status = write_gain(self.device_id, gain);
            if status != 0 {
                bail!("device refused input gain (status {status})");
            }
            // Some devices accept the call but ignore it; verify by reading back.
            match self.get_gain() {
                Some(actual) if (actual - gain).abs() <= 0.02 => Ok(()),
                Some(actual) => bail!("device kept input gain at {:.0}%", actual * 100.0),
                None => bail!("cannot read back input gain"),
            }
        }

        fn term_width(&self) -> usize {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
            if ok && ws.ws_col > 0 {
                ws.ws_col as usize
            } else {
                80
            }
        }

        fn next_chunk(&mut self) -> Result<Vec<f32>> {
            let mut chunk = self
                .rx
                .recv_timeout(Duration::from_millis(500))
                .with_context(|| "audio input timed out")?;
            // accumulate up to 100ms if smaller
            let target_n = (SAMPLE_RATE / 10) as usize;
            while chunk.len() < target_n {
                if let Ok(extra) = self.rx.recv_timeout(Duration::from_millis(50)) {
                    chunk.extend(extra);
                } else {
                    break;
                }
            }
            Ok(chunk)
        }

        fn flush_input(&mut self) {
            // Wait out the buffer that is still filling (it may hold a key
            // click), then drop everything queued so far.
            std::thread::sleep(Duration::from_millis(300));
            while self.rx.try_recv().is_ok() {}
        }

        fn poll_key(&mut self) -> Option<UserKey> {
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut c_void, 1) };
            if n == 1 {
                match buf[0] {
                    b'r' | b'R' | b' ' => Some(UserKey::Rerecord),
                    // Ctrl-C arrives as SIGINT (ISIG stays on), never as a byte.
                    b'q' | b'Q' => Some(UserKey::Quit),
                    b'\r' | b'\n' => Some(UserKey::Enter),
                    b'p' | b'P' => Some(UserKey::Play),
                    _ => None,
                }
            } else {
                None
            }
        }

        fn read_key(&mut self) -> Result<UserKey> {
            loop {
                if let Some(key) = self.poll_key() {
                    return Ok(key);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn play(&mut self, pcm: &[f32], sr: u32) -> Result<()> {
            let temp_dir = tempfile::tempdir()?;
            let temp_path = temp_dir.path().join("preview.wav");
            write_wav16(&temp_path, pcm, sr)?;
            // afplay is native to macOS
            let status = std::process::Command::new("/usr/bin/afplay")
                .arg(&temp_path)
                .status()
                .with_context(|| "playing audio preview via /usr/bin/afplay")?;
            if !status.success() {
                eprintln!("  Warning: afplay failed ({status}); preview not played.");
            }
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacAudioSession;
