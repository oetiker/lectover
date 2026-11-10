//! `lectover voice record` tests: CLI parsing, gain calibration math, onset + pre-roll
//! capture, mid-take and post-take re-recording, and saving to the voice registry.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use lectover::cli::{Cli, Commands, VoiceCommand};
use lectover::audio::denoise::Denoise;
use lectover::design::REF_TEXT;
use lectover::record::{
    check_take_len, default_seconds, fit_status, headset_mic_warning, next_gain, onset_threshold, record_voice, AudioSession,
    RecordRequest, UserKey, SAMPLE_RATE, TEST_TEXT,
};
use lectover::voices::load_registry;

struct FakeSession {
    sr: u32,
    gain: Option<f32>,
    settable: bool,
    chunks: VecDeque<Vec<f32>>,
    poll_keys: VecDeque<Option<UserKey>>,
    menu_keys: VecDeque<UserKey>,
    played: Vec<Vec<f32>>,
    /// Chunks served after the script ran out; guards against endless waits.
    idle: usize,
}

impl FakeSession {
    fn new(sr: u32, gain: Option<f32>) -> Self {
        Self {
            sr,
            gain,
            settable: true,
            chunks: VecDeque::new(),
            poll_keys: VecDeque::new(),
            menu_keys: VecDeque::new(),
            played: Vec::new(),
            idle: 0,
        }
    }

    fn push_silence(&mut self, chunks: usize) {
        let n = (self.sr / 10) as usize; // 100 ms per chunk
        for _ in 0..chunks {
            self.chunks.push_back(vec![0.001; n]);
        }
    }

    fn push_tone(&mut self, chunks: usize, amp: f32) {
        let n = (self.sr / 10) as usize;
        for _ in 0..chunks {
            let chunk: Vec<f32> = (0..n)
                .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / self.sr as f32).sin() * amp)
                .collect();
            self.chunks.push_back(chunk);
        }
    }
}

impl AudioSession for FakeSession {
    fn device_name(&self) -> &str {
        "Fake Mic"
    }
    fn sample_rate(&self) -> u32 {
        self.sr
    }
    fn get_gain(&mut self) -> Option<f32> {
        self.gain
    }
    fn set_gain(&mut self, gain: f32) -> Result<()> {
        if !self.settable {
            anyhow::bail!("device kept input gain");
        }
        self.gain = Some(gain);
        Ok(())
    }
    fn next_chunk(&mut self) -> Result<Vec<f32>> {
        let n = (self.sr / 10) as usize;
        if let Some(chunk) = self.chunks.pop_front() {
            return Ok(chunk);
        }
        self.idle += 1;
        anyhow::ensure!(self.idle < 300, "fake audio script exhausted (waiting forever?)");
        Ok(vec![0.001; n])
    }
    fn flush_input(&mut self) {}
    fn poll_key(&mut self) -> Option<UserKey> {
        self.poll_keys.pop_front().flatten()
    }
    fn read_key(&mut self) -> Result<UserKey> {
        Ok(self.menu_keys.pop_front().unwrap_or(UserKey::Enter))
    }
    fn play(&mut self, pcm: &[f32], _sr: u32) -> Result<()> {
        self.played.push(pcm.to_vec());
        Ok(())
    }
}

/// Returns the take unchanged.
struct PassThrough;
impl Denoise for PassThrough {
    fn denoise(&mut self, pcm: &[f32], _sr: u32) -> Result<Vec<f32>> {
        Ok(pcm.to_vec())
    }
}

/// Halves the take and counts its calls.
#[derive(Default)]
struct Halve {
    calls: usize,
    /// The raw take the last call received.
    last_input: Vec<f32>,
}
impl Denoise for Halve {
    fn denoise(&mut self, pcm: &[f32], _sr: u32) -> Result<Vec<f32>> {
        self.calls += 1;
        self.last_input = pcm.to_vec();
        Ok(pcm.iter().map(|s| s * 0.5).collect())
    }
}

struct Failing;
impl Denoise for Failing {
    fn denoise(&mut self, _pcm: &[f32], _sr: u32) -> Result<Vec<f32>> {
        anyhow::bail!("denoiser exploded")
    }
}

/// A session whose calibration and one 1 s take (plus one extra take per `Rerecord` in `menu`) are scripted.
fn scripted_session(menu: &[UserKey]) -> FakeSession {
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.push_silence(5);
    session.push_tone(40, 0.50);
    session.push_silence(20);
    for _ in menu.iter().filter(|k| **k == UserKey::Rerecord) {
        session.push_tone(11, 0.40);
        session.push_silence(20);
        session.push_silence(5);
    }
    session.push_tone(11, 0.40);
    session.menu_keys.extend(menu.iter().copied());
    session
}

fn read_f32(path: &Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap();
    r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
}

/// Expected (cleaned, raw) outputs for the raw take `input` under the `Halve` denoiser.
fn expected_outputs(input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let halved: Vec<f32> = input.iter().map(|s| s * 0.5).collect();
    let (clean, trim) = lectover::audio::trim_and_level_with(&halved, SAMPLE_RATE);
    (clean, lectover::audio::apply_trim(input, SAMPLE_RATE, &trim))
}

fn assert_close(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    let max = got.iter().zip(want).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(max < 2e-4, "{what}: max deviation {max}");
}

#[test]
fn preview_plays_cleaned_take() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = scripted_session(&[]);
    let mut den = Halve::default();
    record_voice(&mut session, &mut den, voices.path(), &short_req("pv"), &mut Vec::new()).unwrap();
    assert_eq!(den.calls, 1);
    assert_eq!(session.played.len(), 1);
    let (clean, raw) = expected_outputs(&den.last_input);
    assert_close(&session.played[0], &clean, "preview vs cleaned");
    assert!(
        clean.iter().zip(&raw).any(|(a, b)| (a - b).abs() > 1e-3),
        "cleaned and raw must differ for this test to mean anything"
    );
}

#[test]
fn saves_cleaned_and_raw() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = scripted_session(&[]);
    let mut log = Vec::new();
    let mut den = Halve::default();
    let wav = record_voice(&mut session, &mut den, voices.path(), &short_req("both"), &mut log).unwrap();
    let (clean, raw) = expected_outputs(&den.last_input);
    assert_close(&read_f32(&wav), &clean, "<id>.wav vs cleaned");
    assert_close(&read_f32(&voices.path().join("both.raw.wav")), &raw, "<id>.raw.wav vs raw");
    assert!(clean.iter().zip(&raw).any(|(a, b)| (a - b).abs() > 1e-3));
    assert!(String::from_utf8(log).unwrap().contains("  Removing background noise..."));
    let reg = load_registry(voices.path()).unwrap();
    assert_eq!(reg.keys().collect::<Vec<_>>(), vec!["both"]);
    assert!(voices.path().join("both.txt").exists());
}

#[test]
fn rerecord_cleans_each_take() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = scripted_session(&[UserKey::Rerecord, UserKey::Enter]);
    let mut den = Halve::default();
    record_voice(&mut session, &mut den, voices.path(), &short_req("two"), &mut Vec::new()).unwrap();
    assert_eq!(den.calls, 2);
}

#[test]
fn force_keeps_old_raw_too() {
    let voices = tempfile::tempdir().unwrap();
    for ext in ["wav", "raw.wav", "txt"] {
        fs::write(voices.path().join(format!("old.{ext}")), b"x").unwrap();
    }
    fs::write(voices.path().join("voices.json"), r#"{"old": {"label": "Old"}}"#).unwrap();
    let mut session = scripted_session(&[]);
    let mut req = short_req("old");
    req.force = true;
    record_voice(&mut session, &mut Halve::default(), voices.path(), &req, &mut Vec::new()).unwrap();
    let names: Vec<String> = fs::read_dir(voices.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    // `old.wav` -> `old.<stamp>.wav`, `old.raw.wav` -> `old.raw.<stamp>.wav`.
    let stamped = |stem: &str, ext: &str| {
        names.iter().any(|n| {
            n.starts_with(&format!("{stem}.")) && n.ends_with(&format!(".{ext}")) && n.len() == stem.len() + ext.len() + 2 + 15
        })
    };
    assert!(stamped("old", "wav"), "{names:?}");
    assert!(stamped("old.raw", "wav"), "{names:?}");
    assert!(stamped("old", "txt"), "{names:?}");
    assert!(names.contains(&"old.raw.wav".to_string()));
}

#[test]
fn denoise_error_aborts_without_writing() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = scripted_session(&[]);
    let err = record_voice(&mut session, &mut Failing, voices.path(), &short_req("bad"), &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("denoiser exploded"), "{err}");
    assert!(fs::read_dir(voices.path()).unwrap().next().is_none());
}

#[test]
fn cli_voice_record_defaults_and_flags() {
    let cli = Cli::try_parse_from(["lectover", "voice", "record", "meine", "--label", "Meine Stimme"]).unwrap();
    let Commands::Voice(VoiceCommand::Record(a)) = cli.command else {
        panic!("expected VoiceCommand::Record");
    };
    assert_eq!(a.id, "meine");
    assert_eq!(a.label, "Meine Stimme");
    assert!(a.text.is_none());
    assert!(a.seconds.is_none());
    assert!(!a.force);
    assert!(a.voices.is_none());
    assert_eq!(default_seconds(REF_TEXT), 12);
}

#[test]
fn gain_calibration_steps_hardware_and_software_gain() {
    // Hot signal (peak 0.95) -> lowers hardware gain toward target (~0.501)
    let (hw, sw) = next_gain(Some(0.80), 1.0, 0.95);
    let hw = hw.unwrap();
    assert!(hw < 0.50 && hw > 0.35, "expected lowered hw gain, got {hw}");
    assert!((sw - 1.0).abs() < 1e-4, "sw gain stays 1.0 when hw absorbs it: {sw}");

    // Fixed-gain USB mic (hw = None) -> adjusts software gain
    let (hw, sw) = next_gain(None, 1.0, 0.25);
    assert!(hw.is_none());
    assert!(sw > 1.8 && sw < 2.2, "expected ~2x sw gain, got {sw}");
}

#[test]
fn record_flow_supports_mid_take_and_post_take_rerecord() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));

    // Step 1: 5 chunks noise floor (0.5s), 2 chunks silence, 30 chunks test speech (3.0s, hot at 0.90),
    // 1.5 s pause that ends calibration
    session.push_silence(5);
    session.push_silence(2);
    session.push_tone(30, 0.90);
    session.push_silence(15);
    for _ in 0..52 {
        session.poll_keys.push_back(None);
    }

    // Step 2, attempt 1 (aborted mid-take with 'r' after 3 chunks of speech):
    session.push_silence(5);
    session.push_tone(2, 0.40);
    // poll_keys: None during silence (5), None for both speech chunks, then Some(Rerecord)
    for _ in 0..7 {
        session.poll_keys.push_back(None);
    }
    session.poll_keys.push_back(Some(UserKey::Rerecord));

    // Step 2, attempt 2 (completes 1s = 10 chunks, then user presses Rerecord at review menu):
    session.push_silence(5);
    session.push_tone(11, 0.40); // onset chunk + 1 s take
    session.menu_keys.push_back(UserKey::Rerecord);

    // Step 2, attempt 3 (completes 1s = 10 chunks, user presses Play then Enter to save):
    session.push_silence(5);
    session.push_tone(8, 0.40);
    session.push_silence(2);
    session.menu_keys.push_back(UserKey::Play);
    session.menu_keys.push_back(UserKey::Enter);

    let req = RecordRequest {
        id: "test-stimme".into(),
        label: "Test Stimme".into(),
        text: "Ein kurzer Satz.".into(),
        seconds: 1,
        force: false,
    };
    let mut log = Vec::new();
    let wav = record_voice(&mut session, &mut PassThrough, voices.path(), &req, &mut log).unwrap();

    let log_str = String::from_utf8(log).unwrap();
    assert!(log_str.contains(TEST_TEXT), "must show calibration text: {log_str}");
    assert!(log_str.contains("Ein kurzer Satz."), "must show reading text: {log_str}");
    assert!(session.gain.unwrap() < 0.80, "hardware gain should have been reduced from 0.80");
    // Preview played on attempt 2 (1x) + attempt 3 initial (1x) + attempt 3 'p' replay (1x) = 3 plays
    assert_eq!(session.played.len(), 3);

    // Saved WAV is 24 kHz 16-bit mono and trimmed
    let reader = hound::WavReader::open(&wav).unwrap();
    assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().bits_per_sample, 16);
    assert!(reader.duration() > SAMPLE_RATE / 3, "expected speech samples in WAV");
    assert_eq!(
        fs::read_to_string(voices.path().join("test-stimme.txt")).unwrap(),
        "Ein kurzer Satz.\n"
    );
    assert_eq!(load_registry(voices.path()).unwrap()["test-stimme"], "Test Stimme");
}

#[test]
fn voice_record_ctrl_c_cancels_in_step1_and_step2() {
    let voices = tempfile::tempdir().unwrap();
    let req = RecordRequest {
        id: "cancel-test".into(),
        label: "Cancel Test".into(),
        text: "Kurzer Test.".into(),
        seconds: 2,
        force: false,
    };

    // Case 1: Cancel during noise floor measurement (chunk 2)
    {
        let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
        session.push_silence(5);
        session.poll_keys.push_back(None);
        session.poll_keys.push_back(Some(UserKey::Quit));
        let mut log = Vec::new();
        let err = record_voice(&mut session, &mut PassThrough, voices.path(), &req, &mut log).unwrap_err();
        assert!(err.to_string().contains("cancelled by user"), "{err}");
    }

    // Case 2: Cancel while waiting for voice in Step 1
    {
        let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
        session.push_silence(5);
        for _ in 0..5 {
            session.poll_keys.push_back(None);
        }
        session.push_silence(10);
        session.poll_keys.push_back(None);
        session.poll_keys.push_back(Some(UserKey::Quit));
        let mut log = Vec::new();
        let err = record_voice(&mut session, &mut PassThrough, voices.path(), &req, &mut log).unwrap_err();
        assert!(err.to_string().contains("cancelled by user"), "{err}");
    }

    // Case 3: Cancel during countdown in Step 2
    {
        let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
        session.push_silence(5);
        session.push_silence(2);
        session.push_tone(30, 0.50);
        session.push_silence(15);
        for _ in 0..52 {
            session.poll_keys.push_back(None);
        }
        // Step 2: 0.5 s silence arms the trigger, speech starts, cancel on chunk 2 of recording
        session.push_silence(5);
        session.push_tone(10, 0.40);
        for _ in 0..5 {
            session.poll_keys.push_back(None); // waiting
        }
        session.poll_keys.push_back(None); // onset
        session.poll_keys.push_back(Some(UserKey::Quit)); // recording chunk
        let mut log = Vec::new();
        let err = record_voice(&mut session, &mut PassThrough, voices.path(), &req, &mut log).unwrap_err();
        assert!(err.to_string().contains("cancelled by user"), "{err}");
    }
}

#[test]
fn voice_record_cli_refuses_existing_id_before_opening_mic() {
    let voices = tempfile::tempdir().unwrap();
    fs::write(voices.path().join("voices.json"), r#"{"alt": {"label": "Alt"}}"#).unwrap();
    fs::write(voices.path().join("alt.wav"), b"x").unwrap();
    fs::write(voices.path().join("alt.txt"), b"x").unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lectover"))
        .args(["voice", "record", "alt", "--label", "Neu", "--voices"])
        .arg(voices.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--force"), "{err}");
}

fn short_req(id: &str) -> RecordRequest {
    RecordRequest {
        id: id.into(),
        label: "Test".into(),
        text: "Ein kurzer Satz.".into(),
        seconds: 1,
        force: false,
    }
}

#[test]
fn status_lines_fit_the_terminal_width() {
    // The REC line used to be 81 columns and wrapped in an 80-column terminal,
    // which turned every \r update into a new line.
    let rec = "  🔴 REC 00:12 [██████████] -12 dBFS  [r] re-record | Ctrl-C cancel";
    let width = |s: &str| s.chars().map(|c| if c as u32 >= 0x1F300 { 2 } else { 1 }).sum::<usize>();
    assert!(width(rec) < 80, "REC line must fit untruncated in 80 columns");
    assert_eq!(fit_status(rec, 80), rec);
    // Narrow terminal: truncated to width - 1, emoji counted as two columns.
    let cut = fit_status(rec, 20);
    assert!(width(&cut) <= 19, "{cut:?}");
    assert!(rec.starts_with(&cut));
    assert_eq!(fit_status("abc", 0), "abc");
}

#[test]
fn refused_hardware_gain_falls_back_to_software_gain() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.settable = false;
    session.push_silence(5);
    session.push_tone(31, 0.90);
    session.push_silence(20);
    session.push_tone(11, 0.40);
    let mut log = Vec::new();
    record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("sw"), &mut log).unwrap();
    let log_str = String::from_utf8(log).unwrap();
    assert_eq!(session.gain, Some(0.80), "hardware gain untouched");
    assert!(log_str.contains("using software gain"), "{log_str}");
    assert!(log_str.contains("x software"), "{log_str}");
}

#[test]
fn onset_threshold_follows_raised_hardware_gain() {
    // Quiet mic at 10% gain: calibration raises it 10x, so the room noise in
    // the take is 10x louder too and must not trigger the recording.
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.10));
    let n = (SAMPLE_RATE / 10) as usize;
    for _ in 0..5 {
        session.chunks.push_back(vec![0.01; n]);
    }
    session.push_tone(31, 0.05);
    for _ in 0..15 {
        session.chunks.push_back(vec![0.01; n]); // pause ends calibration
    }
    for _ in 0..15 {
        session.chunks.push_back(vec![0.1; n]); // noise at the new gain
    }
    session.push_tone(11, 0.50); // onset chunk + 1 s take
    let mut log = Vec::new();
    record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("quiet"), &mut log).unwrap();
    assert_eq!(session.gain, Some(1.0));
    assert!(onset_threshold(0.1) > 0.1);
    assert!(
        session.chunks.is_empty(),
        "take started on room noise; {} chunks left unread",
        session.chunks.len()
    );
}

#[test]
fn calibration_waits_for_the_whole_test_sentence() {
    // The test sentence takes ~5.5 s to read. Calibration used to stop after
    // a fixed 3 s, and step 2 then started the take on the rest of it.
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.push_silence(5);
    session.push_tone(55, 0.50); // test sentence
    session.push_silence(20); // pause: 1.5 s ends calibration, 0.5 s arms the take
    session.push_tone(11, 0.40); // onset chunk + 1 s take
    let mut log = Vec::new();
    record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("long"), &mut log).unwrap();
    assert!(
        session.chunks.is_empty(),
        "take started on the test sentence; {} chunks left unread",
        session.chunks.len()
    );
}

#[test]
fn take_waits_for_quiet_before_arming_the_onset() {
    // Calibration gives up after 10 s; if the speaker is still talking then,
    // step 2 must not start the take until a short quiet stretch has passed.
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.push_silence(5);
    session.push_tone(120, 0.50); // 12 s of talking, past the calibration cap
    session.push_silence(12);
    session.push_tone(11, 0.40); // the real take
    let mut log = Vec::new();
    record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("talky"), &mut log).unwrap();
    assert!(
        session.chunks.is_empty(),
        "take started while the speaker was still talking; {} chunks left unread",
        session.chunks.len()
    );
}

#[test]
fn soft_syllables_do_not_end_the_calibration() {
    // Quiet mic before calibration: speech peaks at 0.3, soft syllables at
    // 0.03 sit below the absolute onset floor (0.04) but far above the noise.
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.push_silence(5);
    for _ in 0..6 {
        session.push_tone(4, 0.30);
        session.push_tone(12, 0.03); // soft stretch, not a pause
    }
    session.push_silence(20); // the real pause
    session.push_tone(11, 0.40); // take
    let mut log = Vec::new();
    record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("soft"), &mut log).unwrap();
    assert!(
        session.chunks.is_empty(),
        "calibration ended on soft speech; {} chunks left unread",
        session.chunks.len()
    );
}

#[test]
fn take_runs_until_the_speaker_pauses() {
    // The speaker reads slower than the estimate: the take must not end
    // while they are still talking, and must end at the pause afterwards.
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.push_silence(5);
    session.push_tone(40, 0.50);
    session.push_silence(20);
    session.push_tone(30, 0.40); // 3 s take, estimate says 1 s
    session.push_silence(15); // pause ends the take
    session.push_tone(5, 0.40); // must stay unread
    let mut req = short_req("slow");
    req.seconds = 10; // longest take
    let mut log = Vec::new();
    let wav = record_voice(&mut session, &mut PassThrough, voices.path(), &req, &mut log).unwrap();
    assert_eq!(session.chunks.len(), 5, "take must end at the pause");
    let reader = hound::WavReader::open(&wav).unwrap();
    assert!(reader.duration() > SAMPLE_RATE * 2, "take was cut: {} samples", reader.duration());
}

#[test]
fn status_updates_stay_on_one_line_until_the_phase_ends() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(SAMPLE_RATE, Some(0.80));
    session.push_silence(5);
    session.push_tone(40, 0.50);
    session.push_silence(20);
    session.push_tone(11, 0.40);
    let mut log = Vec::new();
    record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("lines"), &mut log).unwrap();
    let log_str = String::from_utf8(log).unwrap();
    // The calibration meter overwrites the "waiting" line instead of
    // starting a new one after "Voice detected!".
    assert!(!log_str.contains("Calibrating level...\x1b[K\n"), "{log_str:?}");
    assert!(!log_str.contains("Recording started...\x1b[K\n"), "{log_str:?}");
}

#[test]
fn take_length_limit_includes_preroll() {
    use lectover::audio::denoise::MAX_CLEAN_SECS;
    use lectover::record::PREROLL_SECS;
    let max = (MAX_CLEAN_SECS - PREROLL_SECS).floor() as u32;
    check_take_len(max).unwrap();
    let err = check_take_len(max + 1).unwrap_err().to_string();
    assert!(err.contains(&format!("{}", max + 1)) && err.contains("90"), "{err}");
    assert!(err.contains("--seconds"), "{err}");
}

#[test]
fn headset_warning_cases() {
    assert!(headset_mic_warning(16_000, Some("Whatever")).is_some());
    assert!(headset_mic_warning(48_000, Some("MacBook Pro Microphone")).is_none());
    let w = headset_mic_warning(48_000, Some("AirPods Pro")).unwrap();
    assert!(w.contains("\"AirPods Pro\"") && w.contains("48000 Hz"));
    assert!(headset_mic_warning(24_000, None).is_none());
    assert!(headset_mic_warning(8_000, None).unwrap().starts_with("  Warning: the microphone runs at 8000 Hz"));
}

#[test]
fn record_voice_warns_about_narrowband_mic() {
    let voices = tempfile::tempdir().unwrap();
    let mut session = FakeSession::new(16_000, Some(0.80));
    session.push_silence(5);
    session.push_tone(40, 0.50);
    session.push_silence(20);
    session.push_tone(11, 0.40);
    let mut log = Vec::new();
    let _ = record_voice(&mut session, &mut PassThrough, voices.path(), &short_req("nb"), &mut log);
    let log = String::from_utf8(log).unwrap();
    let w = log.find("Warning: the microphone").expect("warning missing");
    assert!(w < log.find("Measuring background noise").unwrap());
    assert_eq!(log.matches("Warning: the microphone").count(), 1);
}
