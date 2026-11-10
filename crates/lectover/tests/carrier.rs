//! The carrier ending: every take is synthesised with a carrier phrase after the chunk, and
//! cut in the pause before it, so the model's end-of-speech never falls on the chunk's last
//! sound (it dropped final plosives: "Der Entscheid" without its t).

use anyhow::{bail, Result};
use lectover::ffi::Align;
use lectover::render::{pause_cut, with_carrier, CarrierEnding, Ending, CARRIER, CUT_FADE_SECS};

const SR: u32 = 1000;

fn tone(n: usize) -> Vec<f32> {
    vec![0.5; n]
}

/// `speech` ms of sound, `pause` ms of digital silence, `carrier` ms of sound.
fn take(speech: usize, pause: usize, carrier: usize) -> Vec<f32> {
    let mut w = tone(speech);
    w.extend(vec![0.0; pause]);
    w.extend(tone(carrier));
    w
}

#[test]
fn carrier_follows_the_sentence_end() {
    assert_eq!(with_carrier("Der Entscheid."), format!("Der Entscheid.{CARRIER}"));
    assert_eq!(with_carrier("Wirklich?"), format!("Wirklich?{CARRIER}"));
    assert_eq!(with_carrier("«Nein!»"), format!("«Nein!»{CARRIER}"));
    // a chunk without a sentence end gets a full stop first
    assert_eq!(with_carrier("Der Entscheid "), format!("Der Entscheid.{CARRIER}"));
}

#[test]
fn carrier_is_a_new_section_after_a_long_pause() {
    // measured (a cloned voice, 18 takes): the ellipsis plus the paragraph break gives the
    // steadiest pause before the carrier (330..1120 ms)
    assert_eq!(CARRIER, " …\n\nNeues Kapitel.");
}

#[test]
fn pause_cut_lands_in_the_longest_silence() {
    // 500 ms speech, 400 ms pause, 300 ms carrier: cut 150 ms into the pause at most
    let w = take(500, 400, 300);
    assert_eq!(pause_cut(&w, SR, 0, w.len()), Some(650));
}

#[test]
fn pause_cut_uses_half_of_a_short_pause() {
    let w = take(500, 200, 300);
    assert_eq!(pause_cut(&w, SR, 0, w.len()), Some(600));
}

#[test]
fn pause_cut_rejects_a_pause_too_short_to_cut_in() {
    // a 100 ms gap: the final plosive may still be releasing into the carrier
    let w = take(500, 100, 300);
    assert_eq!(pause_cut(&w, SR, 0, w.len()), None);
}

#[test]
fn pause_cut_only_searches_the_given_range() {
    // an earlier, longer pause inside the chunk is not where the carrier starts
    let mut w = take(300, 600, 300);
    w.extend(take(0, 200, 300));
    assert_eq!(pause_cut(&w, SR, 1000, w.len()), Some(1300));
}

#[test]
fn pause_cut_treats_quiet_noise_as_silence() {
    // room noise 60 dB below the speech counts as pause
    let mut w = tone(500);
    w.extend(vec![0.0005; 400]);
    w.extend(tone(300));
    assert_eq!(pause_cut(&w, SR, 0, w.len()), Some(650));
}

#[test]
fn pause_cut_follows_a_noisier_room() {
    // "Die Debatte." (a cloned voice, seed 42): a 950 ms pause whose noise wobbles around
    // -50 dB below the peak; a fixed -50 dB threshold found no 150 ms of it
    let mut w = tone(500);
    let lo = 0.5 * 10f32.powf(-53.0 / 20.0);
    let hi = 0.5 * 10f32.powf(-48.0 / 20.0);
    w.extend((0..400).map(|i| if (i / 10) % 3 == 0 { hi } else { lo }));
    w.extend(tone(300));
    assert_eq!(pause_cut(&w, SR, 0, w.len()), Some(650));
}

#[test]
fn pause_cut_never_takes_speech_for_pause() {
    // speech dipping to -35 dB is no pause, whatever the window's floor
    let mut w = tone(500);
    w.extend(vec![0.5 * 10f32.powf(-35.0 / 20.0); 400]);
    w.extend(tone(300));
    assert_eq!(pause_cut(&w, SR, 0, w.len()), None);
}

/// Fake aligner: returns fixed spans (seconds) for the words it is given.
struct FakeAlign(Result<Vec<(f64, f64)>, ()>);

impl Align for FakeAlign {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>> {
        assert!(!pcm16k.is_empty());
        match &self.0 {
            Ok(spans) => Ok(words.iter().cloned().zip(spans).map(|(w, &(s, e))| (w, s, e)).collect()),
            Err(()) => bail!("no alignment"),
        }
    }
}

#[test]
fn carrier_ending_cuts_before_the_carrier_and_fades() {
    // "Der Entscheid" 0.0..0.5, pause, "Neues Kapitel" from 0.9
    let w = take(500, 400, 300);
    let mut al = FakeAlign(Ok(vec![(0.0, 0.2), (0.2, 0.5), (0.9, 1.0), (1.0, 1.2)]));
    let mut end = CarrierEnding::new(&mut al);
    assert_eq!(end.tts_text("Der Entscheid."), with_carrier("Der Entscheid."));
    let (cut, clean) = end.finish(&w, SR, "Der Entscheid.").unwrap();
    assert!(clean);
    assert_eq!(cut.len(), 650);
    assert_eq!(cut[..500], w[..500]);
    let fade = (CUT_FADE_SECS * SR as f64) as usize;
    assert!(cut[650 - fade..].iter().all(|&v| v == 0.0), "fade lies in the silence");
}

#[test]
fn carrier_ending_finds_a_pause_the_aligner_gave_to_the_last_word() {
    // "Die Debatte." (a cloned voice): the aligner stretched "Debatte" over the whole pause
    let w = take(500, 400, 300);
    let mut al = FakeAlign(Ok(vec![(0.0, 0.2), (0.2, 0.88), (0.96, 1.0), (1.0, 1.2)]));
    let mut end = CarrierEnding::new(&mut al);
    let (cut, clean) = end.finish(&w, SR, "Die Debatte.").unwrap();
    assert!(clean);
    assert_eq!(cut.len(), 650);
}

#[test]
fn carrier_ending_without_a_pause_cuts_at_the_carrier_and_is_not_clean() {
    let w = take(500, 50, 300);
    let mut al = FakeAlign(Ok(vec![(0.0, 0.2), (0.2, 0.5), (0.55, 0.7), (0.7, 0.85)]));
    let mut end = CarrierEnding::new(&mut al);
    let (cut, clean) = end.finish(&w, SR, "Der Entscheid.").unwrap();
    assert!(!clean);
    assert_eq!(cut.len(), 550);
}

#[test]
fn carrier_ending_without_alignment_falls_back_to_the_last_pause() {
    let w = take(500, 400, 300);
    let mut al = FakeAlign(Err(()));
    let mut end = CarrierEnding::new(&mut al);
    let (cut, clean) = end.finish(&w, SR, "Der Entscheid.").unwrap();
    assert!(!clean, "an unaligned take is never clean");
    assert_eq!(cut.len(), 650);
}

#[test]
fn carrier_ending_with_wrong_word_count_is_not_clean() {
    let w = take(500, 400, 300);
    let mut al = FakeAlign(Ok(vec![(0.0, 0.5)])); // zip truncates: one word back
    let mut end = CarrierEnding::new(&mut al);
    let (_, clean) = end.finish(&w, SR, "Der Entscheid.").unwrap();
    assert!(!clean);
}
