//! Qwen3-TTS voice cloning via qwentts.cpp (`qwen.h`).

use std::ffi::CString;
use std::path::Path;

use sha1::{Digest, Sha1};

use anyhow::{anyhow, bail, Context, Result};
use lectover_sys as sys;

use super::backend::{check_ggml_layout, ggml_device_name};
use super::{c_len, c_path, c_str, c_string, require_file, Device, Synth, TakeRequest};
use crate::audio::{quiet_ending, read_wav, resample};

/// The codec's output rate (qt_audio.sample_rate); voice references are fed at this rate too.
const TTS_RATE: u32 = 24000;
/// Codec frames decoded after end-of-speech, filled with the voice clip's last frame
/// (room tone), so the causal decoder rings out the last syllable instead of the audio
/// stopping mid-decay. Measured: the decay reaches room tone within 60-80 ms.
const TAIL_PAD_FRAMES: i32 = 3;
/// Samples per codec frame at `TTS_RATE` (12.5 Hz codec).
const CODEC_HOP: usize = 1920;
/// qwentts.cpp's QT_DEFAULT_SUBTALKER_TEMPERATURE.
pub const DEFAULT_SUBTALKER_TEMPERATURE: f32 = 0.9;
/// Language name as `qt_language_name` spells it.
const LANG: &str = "german";

/// qwentts.cpp handle (talker + codec) plus the current clone voice (ICL: reference clip and
/// its transcript). With `max_batch > 1`, `generate` runs its texts concurrently and the
/// library batches them into shared decode steps.
pub struct QwenTts {
    ctx: *mut sys::qt_context,
    /// Concurrent sequences the handle decodes (qt_init_params.max_batch).
    max_batch: usize,
    /// The clone voice, encoded once (speaker embedding + codec codes) instead of on
    /// every take: the encode cost ~0.7 s per take, 9 % of synthesis time.
    voice: VoiceRef,
    /// Library version, model files and device: part of [`Synth::fingerprint`].
    model_id: String,
    /// Hash of the voice clip and its transcript: part of [`Synth::fingerprint`].
    voice_id: String,
    voice_text: CString,
    lang: CString,
    /// Sampling temperature of the code predictor (residual codebooks: timbre, room, noise).
    subtalker_temperature: f32,
    /// Whole text in the prompt instead of the streaming text feed (qwentts ABI 8).
    non_streaming: bool,
}

// SAFETY: the handle is not tied to the creating thread; qwen.h documents qt_synthesize as
// thread safe, and all calls go through `&mut self` anyway.
unsafe impl Send for QwenTts {}

/// Shares a handle with the scoped threads of one `generate` call.
struct Shared<'a>(&'a QwenTts);
// SAFETY: those threads only call qt_synthesize (thread safe per qwen.h) and read the voice
// and strings, which nothing mutates while `generate` holds `&mut QwenTts`.
unsafe impl Sync for Shared<'_> {}

impl Shared<'_> {
    // A method, not `this.0.synthesize`: closures capture disjoint fields, and `&QwenTts`
    // itself is not Send.
    fn synthesize(
        &self,
        text: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> Result<Vec<f32>> {
        self.0.synthesize(text, max_tokens, seed, temperature)
    }
}

/// Owns a `qt_voice_ref` filled by `qt_extract_voice_ref`.
struct VoiceRef(sys::qt_voice_ref);

impl VoiceRef {
    fn empty() -> Self {
        VoiceRef(sys::qt_voice_ref {
            ref_spk_emb: std::ptr::null_mut(),
            ref_spk_dim: 0,
            ref_codes: std::ptr::null_mut(),
            ref_T: 0,
            num_codebooks: 0,
        })
    }
}

impl Drop for VoiceRef {
    fn drop(&mut self) {
        // SAFETY: qt_voice_ref_free is safe on an empty struct and on one qt_extract_voice_ref filled.
        unsafe { sys::qt_voice_ref_free(&mut self.0) };
    }
}

impl Drop for QwenTts {
    fn drop(&mut self) {
        // SAFETY: ctx came from qt_init and is freed exactly once (qt_free is NULL-safe).
        unsafe { sys::qt_free(self.ctx) };
    }
}

/// Owns a `qt_audio` filled by `qt_synthesize`; releases its buffer on every path.
struct Audio(sys::qt_audio);

impl Drop for Audio {
    fn drop(&mut self) {
        // SAFETY: qt_audio_free is safe on a zeroed struct and on one qt_synthesize filled.
        unsafe { sys::qt_audio_free(&mut self.0) };
    }
}

fn last_error() -> String {
    // SAFETY: qt_last_error returns a thread-local NUL-terminated string, never freed by us.
    c_str(unsafe { sys::qt_last_error() })
}

fn check_abi(found: i32, what: &str) -> Result<()> {
    if found != sys::QT_ABI_VERSION as i32 {
        bail!(
            "qwentts.cpp ABI mismatch: {what} has abi_version {found}, bindings expect {}",
            sys::QT_ABI_VERSION
        );
    }
    Ok(())
}

/// Load a talker + codec pair on `device` and check its model type and German support.
/// The returned handle is freed by the caller's `Drop`.
fn init(
    talker: &Path,
    codec: &Path,
    device: Device,
    model_type: &str,
    max_batch: usize,
) -> Result<*mut sys::qt_context> {
    check_ggml_layout()?;
    require_file(talker, "TTS talker model")?;
    require_file(codec, "TTS codec model")?;
    let talker_c = c_path(talker)?;
    let codec_c = c_path(codec)?;
    let device_c = c_string(&ggml_device_name(device)?, "device name")?;

    let mut params = sys::qt_init_params::default();
    // SAFETY: params is a valid, writable qt_init_params.
    unsafe { sys::qt_init_default_params(&mut params) };
    check_abi(params.abi_version, "qt_init_params")?;
    params.talker_path = talker_c.as_ptr();
    params.codec_path = codec_c.as_ptr();
    params.device = device_c.as_ptr();
    params.max_batch =
        i32::try_from(max_batch.max(1)).map_err(|_| anyhow!("max_batch {max_batch} too large"))?;

    // SAFETY: params and the strings it points to outlive the call; qt_init copies what it keeps.
    let ctx = unsafe { sys::qt_init(&params) };
    if ctx.is_null() {
        bail!(
            "qt_init failed ({} / {}): {}",
            talker.display(),
            codec.display(),
            last_error()
        );
    }
    let checked = (|| {
        // SAFETY: the returned strings are owned by the live handle and copied immediately.
        let found = c_str(unsafe { sys::qt_model_type(ctx) });
        if found != model_type {
            bail!(
                "{}: model type {found:?}, expected a {model_type:?} talker",
                talker.display()
            );
        }
        // SAFETY: as above; i stays in [0, qt_n_languages).
        let langs: Vec<String> = unsafe {
            (0..sys::qt_n_languages(ctx))
                .map(|i| c_str(sys::qt_language_name(ctx, i)))
                .collect()
        };
        if !langs.iter().any(|l| l == LANG) {
            bail!(
                "{}: no {LANG:?} in its languages {langs:?}",
                talker.display()
            );
        }
        Ok(())
    })();
    if let Err(e) = checked {
        // SAFETY: ctx came from qt_init and is not used afterwards.
        unsafe { sys::qt_free(ctx) };
        return Err(e);
    }
    Ok(ctx)
}

/// What conditions a synthesis besides the text: a clone voice (base) or a description
/// (voice_design).
enum Conditioning<'a> {
    Clone {
        voice: &'a sys::qt_voice_ref,
        text: &'a CString,
    },
    Instruct(&'a CString),
}

/// One buffered `qt_synthesize` call on a live handle.
fn synthesize(
    ctx: *mut sys::qt_context,
    lang: &CString,
    cond: Conditioning,
    text: &str,
    max_tokens: usize,
    seed: u64,
    temperature: f32,
    subtalker_temperature: Option<f32>,
    non_streaming: bool,
) -> Result<Vec<f32>> {
    let text_c = c_string(text, "text")?;
    let mut params = sys::qt_tts_params::default();
    // SAFETY: params is a valid, writable qt_tts_params.
    unsafe { sys::qt_tts_default_params(&mut params) };
    check_abi(params.abi_version, "qt_tts_params")?;
    params.text = text_c.as_ptr();
    params.lang = lang.as_ptr();
    match cond {
        Conditioning::Clone { voice, text } => {
            params.ref_spk_emb = voice.ref_spk_emb;
            params.ref_spk_dim = voice.ref_spk_dim;
            params.ref_codes = voice.ref_codes;
            params.ref_T = voice.ref_T;
            params.ref_text = text.as_ptr();
            params.tail_pad_frames = TAIL_PAD_FRAMES;
            params.icl_non_streaming = non_streaming as i32;
        }
        Conditioning::Instruct(instruct) => params.instruct = instruct.as_ptr(),
    }
    // -1 would mean "random seed" to the library.
    params.seed = i64::try_from(seed).map_err(|_| anyhow!("seed {seed} exceeds i64"))?;
    params.temperature = temperature;
    if let Some(t) = subtalker_temperature {
        params.subtalker_temperature = t;
    }
    params.max_new_tokens = c_len(max_tokens, "max_tokens")?;

    let mut out = Audio(sys::qt_audio {
        samples: std::ptr::null_mut(),
        n_samples: 0,
        sample_rate: 0,
        channels: 0,
    });
    // SAFETY: ctx is live; params points at buffers (text, voice or instruct, lang) that
    // outlive this blocking call; out is a zeroed qt_audio that Audio's Drop releases.
    let status = unsafe { sys::qt_synthesize(ctx, &params, &mut out.0) };
    if status != sys::QT_STATUS_OK {
        bail!("qt_synthesize failed (status {status}): {}", last_error());
    }
    let a = &out.0;
    if a.sample_rate != TTS_RATE as i32 || a.channels != 1 {
        bail!(
            "qt_synthesize returned {} Hz / {} channels, expected {TTS_RATE} Hz mono",
            a.sample_rate,
            a.channels
        );
    }
    let n = usize::try_from(a.n_samples)
        .map_err(|_| anyhow!("negative sample count {}", a.n_samples))?;
    if n == 0 || a.samples.is_null() {
        bail!("qt_synthesize returned no audio for {text:?}");
    }
    // SAFETY: on success samples points at n_samples floats owned by `out` until its Drop.
    Ok(unsafe { std::slice::from_raw_parts(a.samples, n) }.to_vec())
}

impl QwenTts {
    /// Load the talker and codec GGUFs on `device` and set the clone voice. `max_batch`
    /// sequences are decoded together (1 = one take at a time).
    pub fn load(
        talker: &Path,
        codec: &Path,
        voice_wav: &Path,
        voice_text: &str,
        device: Device,
        max_batch: usize,
    ) -> Result<Self> {
        let max_batch = max_batch.max(1);
        let ctx = init(talker, codec, device, "base", max_batch)?;
        // SAFETY: qt_version returns a static string.
        let version = c_str(unsafe { sys::qt_version() });
        let file_id = |p: &Path| -> String {
            let meta = std::fs::metadata(p).ok();
            let len = meta.as_ref().map_or(0, |m| m.len());
            let mtime = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{}:{len}:{mtime}", p.display())
        };
        let model_id = format!(
            "qwentts {version} | {} | {} | {device}",
            file_id(talker),
            file_id(codec)
        );
        let mut tts = QwenTts {
            ctx,
            max_batch,
            voice: VoiceRef::empty(),
            model_id,
            voice_id: String::new(),
            voice_text: CString::default(),
            lang: c_string(LANG, "language")?,
            subtalker_temperature: DEFAULT_SUBTALKER_TEMPERATURE,
            non_streaming: false,
        };
        tts.set_voice(voice_wav, voice_text)?;
        Ok(tts)
    }

    /// Switch the clone voice: reference clip (any rate, mixed to mono) and its transcript.
    /// Set the code predictor's sampling temperature (library default 0.9).
    pub fn set_subtalker_temperature(&mut self, t: f32) {
        self.subtalker_temperature = t;
    }

    /// Put the whole text in the prompt (upstream `non_streaming_mode=True`) instead of
    /// feeding it one token per generated frame.
    pub fn set_non_streaming(&mut self, on: bool) {
        self.non_streaming = on;
    }

    pub fn set_voice(&mut self, wav: &Path, text: &str) -> Result<()> {
        let (pcm, sr) = read_wav(wav).with_context(|| format!("voice {}", wav.display()))?;
        if pcm.is_empty() {
            bail!("voice clip {} is empty", wav.display());
        }
        let text = text.trim();
        if text.is_empty() {
            bail!("voice {} has an empty transcript", wav.display());
        }
        let pcm = resample(&pcm, sr, TTS_RATE);
        c_len(pcm.len(), "voice clip")?;
        if !quiet_ending(&pcm, TTS_RATE) {
            bail!(
                "voice clip {} must end in at least 0.16 s of quiet room tone: takes are padded with its last frame",
                wav.display()
            );
        }
        let mut voice = VoiceRef::empty();
        // SAFETY: ctx is live, pcm outlives the blocking call, voice.0 is an empty struct
        // that the call fills with malloc-owned buffers VoiceRef's Drop releases.
        let status = unsafe {
            sys::qt_extract_voice_ref(
                self.ctx,
                pcm.as_ptr(),
                c_len(pcm.len(), "voice clip")?,
                &mut voice.0,
            )
        };
        if status != sys::QT_STATUS_OK {
            bail!(
                "encoding voice clip {} failed (status {status}): {}",
                wav.display(),
                last_error()
            );
        }
        let mut h = Sha1::new();
        h.update(
            pcm.iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        h.update(text.as_bytes());
        self.voice_id = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        self.voice_text = c_string(text, "voice transcript")?;
        self.voice = voice;
        Ok(())
    }

    fn synthesize(
        &self,
        text: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> Result<Vec<f32>> {
        let cond = Conditioning::Clone {
            voice: &self.voice.0,
            text: &self.voice_text,
        };
        synthesize(
            self.ctx,
            &self.lang,
            cond,
            text,
            max_tokens,
            seed,
            temperature,
            Some(self.subtalker_temperature),
            self.non_streaming,
        )
    }
}

impl Synth for QwenTts {
    fn sample_rate(&self) -> u32 {
        TTS_RATE
    }

    fn fingerprint(&self) -> Option<String> {
        Some(format!(
            "{} | voice {} | lang {:?} | subtalker {} | non_streaming {} | tail_pad {TAIL_PAD_FRAMES} | max_batch {}",
            self.model_id,
            self.voice_id,
            self.lang,
            self.subtalker_temperature.to_bits(),
            self.non_streaming,
            self.max_batch
        ))
    }

    fn tail_pad_samples(&self) -> usize {
        TAIL_PAD_FRAMES as usize * CODEC_HOP
    }

    fn max_batch(&self) -> usize {
        self.max_batch
    }

    /// `qt_synthesize` takes one text per call (spike log). With `max_batch > 1` every
    /// request gets its own blocking call on a scoped thread; the library's worker coalesces
    /// them into batched decode steps and queues the rest FIFO.
    fn generate(&mut self, reqs: &[TakeRequest], temperature: f32) -> Result<Vec<Vec<f32>>> {
        self.generate_each(reqs, temperature, &mut |_, _| Ok(()))
    }

    /// Each finished take goes to `done` while the worker keeps decoding the others, so the
    /// caller can check it in the gaps of the TTS decode.
    fn generate_each(
        &mut self,
        reqs: &[TakeRequest],
        temperature: f32,
        done: &mut dyn FnMut(usize, &[f32]) -> Result<()>,
    ) -> Result<Vec<Vec<f32>>> {
        if self.max_batch == 1 || reqs.len() < 2 {
            let mut takes = Vec::with_capacity(reqs.len());
            for (i, r) in reqs.iter().enumerate() {
                let t = self.synthesize(&r.text, r.max_tokens, r.seed, temperature)?;
                done(i, &t)?;
                takes.push(t);
            }
            return Ok(takes);
        }
        let this = Shared(self);
        let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Vec<f32>>)>();
        std::thread::scope(|s| {
            for (i, r) in reqs.iter().enumerate() {
                let (this, tx) = (&this, tx.clone());
                s.spawn(move || {
                    let t = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        this.synthesize(&r.text, r.max_tokens, r.seed, temperature)
                    }))
                    .unwrap_or_else(|_| Err(anyhow!("synthesis thread panicked")));
                    // The receiver outlives every sender.
                    let _ = tx.send((i, t));
                });
            }
            drop(tx);
            let mut takes: Vec<Option<Vec<f32>>> = vec![None; reqs.len()];
            // Keep draining after an error: the scope joins every decode anyway.
            let mut first_err: Option<anyhow::Error> = None;
            for (i, t) in rx {
                match t {
                    Ok(t) => {
                        if first_err.is_none() {
                            if let Err(e) = done(i, &t) {
                                first_err = Some(e);
                            }
                        }
                        takes[i] = Some(t);
                    }
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(takes
                    .into_iter()
                    .map(|t| t.expect("every thread sent its take"))
                    .collect()),
            }
        })
    }
}

/// qwentts.cpp handle on a VoiceDesign talker: speaks a text in a voice described in words
/// (`instruct`). Used once per new voice by `voice design`; the render clones with [`QwenTts`].
pub struct QwenVoiceDesign {
    ctx: *mut sys::qt_context,
    lang: CString,
}

// SAFETY: as for QwenTts.
unsafe impl Send for QwenVoiceDesign {}

impl Drop for QwenVoiceDesign {
    fn drop(&mut self) {
        // SAFETY: ctx came from qt_init and is freed exactly once.
        unsafe { sys::qt_free(self.ctx) };
    }
}

impl QwenVoiceDesign {
    /// Load a `voice_design` talker and the codec on `device`.
    pub fn load(talker: &Path, codec: &Path, device: Device) -> Result<Self> {
        Ok(QwenVoiceDesign {
            ctx: init(talker, codec, device, "voice_design", 1)?,
            lang: c_string(LANG, "language")?,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        TTS_RATE
    }

    /// Speak `text` (German) in the voice `instruct` describes; at most `max_tokens` frames.
    pub fn design(
        &mut self,
        text: &str,
        instruct: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> Result<Vec<f32>> {
        if instruct.trim().is_empty() {
            bail!("voice design needs a non-empty voice description");
        }
        let instruct = c_string(instruct, "voice description")?;
        synthesize(
            self.ctx,
            &self.lang,
            Conditioning::Instruct(&instruct),
            text,
            max_tokens,
            seed,
            temperature,
            None,
            false,
        )
    }
}
