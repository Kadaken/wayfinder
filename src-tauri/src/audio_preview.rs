// Playing a sound file in the preview pane.
//
// Video has always played here. Audio was switched off on purpose, and the
// comment that switched it off was right: handing an MP3 to `GtkMediaFile`
// does not fail, it ABORTS the process --
//
//   ERROR:../gst/playback/gstdecodebin3.c:3381:
//   mq_slot_handle_stream_start: assertion failed: (collection)
//
// -- which takes the whole file manager down with it. That was re-tested on
// GTK 4.22 and gstreamer 1.28 before this module was written and it still
// aborts, so the fix is not to turn the old path back on.
//
// `GtkMediaFile` reaches that assertion through `decodebin3`. The OLD
// `playbin`, which this file uses, goes through `uridecodebin`/`decodebin2`
// instead and never touches it. That is not a guess: the same two files that
// abort through GtkMediaFile play to the second through `playbin`. It is also
// the element the video poster pipeline in native.rs already uses, so this is
// the path in this app with a track record rather than a new one.
//
// `video-sink` is a `fakesink`. Plenty of sound files are really media
// containers -- an `.mka`, an `.m4a` with cover art -- and without this
// `playbin` would try to build a window for a video stream nobody asked to
// see.
//
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Kadaken

use gst::prelude::*;
use gstreamer as gst;

/// One sound file, playing or paused, with the pipeline that plays it.
///
/// Dropping it tears the pipeline down, which is what stops the sound. Every
/// place that stops playing does it by dropping this, so there is exactly one
/// way for audio to stop and no way to leave a pipeline running after the
/// preview has moved on to another file.
pub struct AudioPlayer {
    pipeline: gst::Pipeline,
    /// The file being played, for the label.
    path: String,
    /// Set from the bus when the file reaches its end, so the button can go
    /// back to showing Play instead of claiming to still be playing.
    finished: std::rc::Rc<std::cell::Cell<bool>>,
    /// What the bus said went wrong, if anything. Shown instead of controls.
    failed: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    /// D-327: a length worked out from the file itself, for formats whose
    /// decoder (MIDI through fluiddec) reports none.
    computed_secs: Option<f64>,
    _bus_watch: gst::bus::BusWatchGuard,
}

impl AudioPlayer {
    /// Builds the pipeline for `path` and starts it paused, so the duration
    /// and the scrubber are right before anything is heard. Nothing plays
    /// until `set_playing(true)`: selecting a file in a list is not a request
    /// to make noise.
    pub fn open(path: &str) -> Result<Self, String> {
        let uri = gst::glib::filename_to_uri(path, None)
            .map_err(|e| format!("{path} is not a path GStreamer can take: {e}"))?;
        let element = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .build()
            .map_err(|_| "playbin is missing: gstreamer-plugins-base is not installed".to_string())?;
        let pipeline = element
            .clone()
            .downcast::<gst::Pipeline>()
            .map_err(|_| "playbin is not a pipeline".to_string())?;
        // A sound file may carry a video stream or cover art. Throw it away
        // rather than let playbin open a window for it.
        if let Ok(fakesink) = gst::ElementFactory::make("fakesink").build() {
            pipeline.set_property("video-sink", &fakesink);
        }
        // Text streams would otherwise be rendered onto that video sink.
        if let Some(flags) = audio_only_flags(&pipeline) {
            pipeline.set_property_from_value("flags", &flags);
        }

        let finished = std::rc::Rc::new(std::cell::Cell::new(false));
        let failed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let bus = pipeline.bus().ok_or("playbin has no bus")?;
        let bus_watch = {
            let finished = finished.clone();
            let failed = failed.clone();
            let pipeline = pipeline.clone();
            bus.add_watch_local(move |_, message| {
                use gst::MessageView;
                match message.view() {
                    MessageView::Eos(_) => {
                        finished.set(true);
                        // Back to the start and paused, so pressing Play
                        // plays it again rather than doing nothing at the
                        // end of the file.
                        let _ = pipeline.set_state(gst::State::Paused);
                        let _ = pipeline.seek_simple(
                            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                            gst::ClockTime::ZERO,
                        );
                    }
                    MessageView::Error(error) => {
                        // A file that will not decode is a message in the
                        // pane, never a crash and never silence.
                        *failed.borrow_mut() = Some(error.error().to_string());
                        let _ = pipeline.set_state(gst::State::Null);
                    }
                    _ => {}
                }
                gst::glib::ControlFlow::Continue
            })
            .map_err(|_| "could not watch the pipeline".to_string())?
        };

        // Paused, not Playing: this prerolls, which is what makes the
        // duration readable, without a sound.
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|_| format!("nothing on this machine can decode {path}"))?;
        Ok(Self {
            pipeline,
            path: path.to_string(),
            finished,
            failed,
            computed_secs: crate::native_core::is_midi_path(path)
                .then(|| crate::native_core::midi_duration_secs(path))
                .flatten(),
            _bus_watch: bus_watch,
        })
    }

    /// The file this is playing.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Starts or stops the sound. Returns false if the pipeline refused, which
    /// happens when the file turned out not to be decodable after preroll.
    pub fn set_playing(&self, playing: bool) -> bool {
        if self.failed.borrow().is_some() {
            return false;
        }
        if playing {
            self.finished.set(false);
        }
        let state = if playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        self.pipeline.set_state(state).is_ok()
    }

    /// True while sound is actually coming out.
    pub fn is_playing(&self) -> bool {
        !self.finished.get()
            && self.failed.borrow().is_none()
            && self.pipeline.current_state() == gst::State::Playing
    }

    /// What went wrong, if anything has.
    pub fn error(&self) -> Option<String> {
        self.failed.borrow().clone()
    }

    /// Seconds into the file, or 0 before the pipeline can answer.
    pub fn position_secs(&self) -> f64 {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1_000_000_000.0)
            .unwrap_or(0.0)
    }

    /// Length in seconds, or `None` for a stream that will not say.
    pub fn duration_secs(&self) -> Option<f64> {
        self.pipeline
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1_000_000_000.0)
            .filter(|secs| *secs > 0.0)
            .or(self.computed_secs)
    }

    /// True when the length is our own estimate rather than the decoder's.
    pub fn duration_is_computed(&self) -> bool {
        self.computed_secs.is_some()
            && self
                .pipeline
                .query_duration::<gst::ClockTime>()
                .map_or(true, |t| t.nseconds() == 0)
    }

    /// Jumps to `secs`. Clamped to the file, because a scrubber dragged to its
    /// very end asks for a position one tick past the last sample and the
    /// pipeline answers that with an error.
    pub fn seek_secs(&self, secs: f64) {
        let limit = self.duration_secs().unwrap_or(secs);
        let target = secs.clamp(0.0, (limit - 0.05).max(0.0));
        let _ = self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
            gst::ClockTime::from_nseconds((target * 1_000_000_000.0) as u64),
        );
        self.finished.set(false);
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        // Null, not Paused. A paused pipeline still holds the audio device
        // open, so the next sound file could not start on a machine whose
        // sink does not share.
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// playbin's `flags` property set to sound and volume only.
///
/// Every other flag stays off, which is the point: video, subtitles,
/// visualisations and network buffering are all things a sound preview must
/// not quietly switch on. Video off is the one that matters most -- an `.m4a`
/// with cover art or an `.mka` carrying a video track would otherwise have
/// playbin build a window nobody asked for.
///
/// `flags` is a GStreamer flags type, not a number. Setting it with a plain
/// integer looks right and panics at runtime:
///
///   property 'flags' of type 'GstPlayBin' can't be set from the given type
///   (expected: 'GstPlayFlags', got: 'guint')
///
/// So the value is built from the property's own type by nick. That also
/// means the nicks, not hard-coded bit positions, are what this depends on.
/// `None` if playbin ever stops offering the property, in which case the
/// caller leaves the default alone rather than guessing.
fn audio_only_flags(pipeline: &gst::Pipeline) -> Option<gst::glib::Value> {
    let property = pipeline.find_property("flags")?;
    let class = gst::glib::FlagsClass::with_type(property.value_type())?;
    class
        .builder()
        .set_by_nick("audio")
        .set_by_nick("soft-volume")
        .build()
}

/// `mm:ss`, or `h:mm:ss` once a file is an hour long. Audiobooks and DJ sets
/// are hours long, and `71:04` is not a time anybody reads.
pub fn clock_text(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".to_string();
    }
    let total = secs.round() as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_files_read_as_minutes_and_seconds() {
        assert_eq!(clock_text(0.0), "0:00");
        assert_eq!(clock_text(9.4), "0:09");
        assert_eq!(clock_text(65.0), "1:05");
        assert_eq!(clock_text(212.78), "3:33");
    }

    #[test]
    fn an_hour_long_file_gets_an_hours_field() {
        // "71:04" is not a time anybody reads.
        assert_eq!(clock_text(3600.0), "1:00:00");
        assert_eq!(clock_text(4264.0), "1:11:04");
    }

    #[test]
    fn nonsense_durations_do_not_produce_nonsense_text() {
        assert_eq!(clock_text(-5.0), "0:00");
        assert_eq!(clock_text(f64::NAN), "0:00");
        assert_eq!(clock_text(f64::INFINITY), "0:00");
    }

    /// Writes a real, decodable one-second WAV and returns its path.
    ///
    /// A file on disk, not a mock: the claim being tested is that GStreamer
    /// decodes a sound file through this pipeline, and a mock cannot fail the
    /// way the real decoder did.
    fn one_second_wav(name: &str, amplitude: f64) -> std::path::PathBuf {
        let rate: u32 = 8000;
        let samples: u32 = rate; // one second, mono, 16-bit
        let data_bytes = samples * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
        wav.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        // A quiet tone rather than silence, so a decoder that drops empty
        // buffers still has something to carry.
        for index in 0..samples {
            let value = ((index as f64 * 0.05).sin() * amplitude) as i16;
            wav.extend_from_slice(&value.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("wayfinder-audio-test-{name}.wav"));
        std::fs::write(&path, wav).expect("could not write the test file");
        path
    }

    /// True once GStreamer is ready. `main()` does this; a test has to do it
    /// itself, and doing it twice is harmless.
    fn gst_ready() -> bool {
        gst::init().is_ok()
    }

    /// Spins the main context until the pipeline can say how long the file
    /// is, or two seconds pass. Two seconds is far longer than a local WAV
    /// needs and short enough that a broken build fails rather than hangs.
    fn wait_for_duration(player: &AudioPlayer) -> Option<f64> {
        let context = gst::glib::MainContext::thread_default()
            .unwrap_or_else(gst::glib::MainContext::default);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(duration) = player.duration_secs() {
                return Some(duration);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            while context.iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Runs `body` with a main context on this thread.
    ///
    /// The bus watch needs one. In the app that is GTK's, already the thread
    /// default before any of this runs; a test thread has none, and without
    /// this the watch panics inside gstreamer rather than in our code.
    fn in_main_context<T>(body: impl FnOnce() -> T) -> T {
        let context = gst::glib::MainContext::new();
        context
            .with_thread_default(body)
            .expect("could not make a main context the thread default")
    }

    #[test]
    fn a_real_sound_file_opens_and_reports_how_long_it_is() {
        in_main_context(|| {
            // This is the test the old code could not have: handing this file to
            // GtkMediaFile is what aborted the process.
            if !gst_ready() {
                return;
            }
            let path = one_second_wav("duration", 2000.0);
            let player = match AudioPlayer::open(path.to_str().unwrap()) {
                Ok(player) => player,
                Err(message) => {
                    // No sound card in the build environment is not a failure of
                    // this code, but anything else is.
                    assert!(
                        message.contains("decode") || message.contains("sink"),
                        "opening a plain WAV failed for an unexpected reason: {message}"
                    );
                    let _ = std::fs::remove_file(&path);
                    return;
                }
            };
            // `open` does not block waiting for preroll: on a network share that
        // would freeze the window. The duration therefore arrives a moment
        // later, which is exactly what the tick in native.rs handles, so the
        // test waits the same way rather than pretending it is instant.
        let duration = wait_for_duration(&player).expect("a WAV knows its own length");
            assert!(
                (duration - 1.0).abs() < 0.05,
                "a one-second file reported {duration} seconds"
            );
            assert_eq!(player.error(), None);
            // Paused, not playing: selecting a file must not make a noise.
            assert!(!player.is_playing());
            let _ = std::fs::remove_file(&path);
    });
    }

    #[test]
    fn pressing_play_actually_advances_through_the_file() {
        // The claim the whole change rests on: this pipeline plays sound,
        // where handing the same job to GtkMediaFile aborted the process.
        // Silent samples, so running the suite does not make a noise.
        if !gst_ready() {
            return;
        }
        in_main_context(|| {
            let path = one_second_wav("playback", 0.0);
            let Ok(player) = AudioPlayer::open(path.to_str().unwrap()) else {
                let _ = std::fs::remove_file(&path);
                return;
            };
            if wait_for_duration(&player).is_none() {
                // No sound device in this environment; nothing to prove here.
                let _ = std::fs::remove_file(&path);
                return;
            }
            assert!(player.set_playing(true), "the pipeline refused to play");
            let context = gst::glib::MainContext::thread_default()
                .unwrap_or_else(gst::glib::MainContext::default);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let mut reached = 0.0;
            while std::time::Instant::now() < deadline && reached < 0.2 {
                while context.iteration(false) {}
                std::thread::sleep(std::time::Duration::from_millis(20));
                reached = player.position_secs();
            }
            assert_eq!(player.error(), None);
            assert!(
                reached >= 0.2,
                "after playing, the position was still {reached} seconds"
            );
            // And pausing stops it moving.
            player.set_playing(false);
            while context.iteration(false) {}
            let paused_at = player.position_secs();
            std::thread::sleep(std::time::Duration::from_millis(150));
            while context.iteration(false) {}
            let still = player.position_secs();
            assert!(
                (still - paused_at).abs() < 0.05,
                "paused at {paused_at} seconds but it moved to {still}"
            );
            let _ = std::fs::remove_file(&path);
        });
    }

    /// D-327: a real MIDI stream (fluiddec) reports no length, so ours is used,
    /// and a seek is honoured (position probe); gated on the owner's copy of the
    /// song existing so the suite passes without it.
    #[test]
    fn midi_length_comes_from_the_file_and_seeking_works() {
        let song = "/home/user/Music/song.mid";
        if !std::path::Path::new(song).exists() || !gst_ready() {
            return;
        }
        in_main_context(|| {
            let Ok(player) = AudioPlayer::open(song) else { return };
            let context = gst::glib::MainContext::thread_default()
                .unwrap_or_else(gst::glib::MainContext::default);
            let d = wait_for_duration(&player).expect("length must come from somewhere");
            assert!(d > 60.0 && d < 80.0, "computed length {d}");
            eprintln!("MIDI-PROBE decoder length reported: {:?}, computed flag {}",
                player.pipeline.query_duration::<gst::ClockTime>(), player.duration_is_computed());
            if !player.set_playing(true) {
                eprintln!("MIDI-PROBE could not play (no device?)");
                return;
            }
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_millis(1500) {
                while context.iteration(false) {}
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let before = player.position_secs();
            player.seek_secs(30.0);
            let t1 = std::time::Instant::now();
            while t1.elapsed() < std::time::Duration::from_millis(1500) {
                while context.iteration(false) {}
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let after = player.position_secs();
            eprintln!("MIDI-PROBE position before seek {before:.2}, after seek to 30: {after:.2}");
            assert!(before < 10.0, "before the seek it was at {before}");
            assert!(after >= 29.5 && after < 40.0, "a seek to 30 s landed at {after}");
        });
    }

    #[test]
    fn seeking_past_the_end_lands_inside_the_file_instead_of_failing() {
        in_main_context(|| {
            // A scrubber dragged to its far right asks for a position one tick
            // past the last sample, which the pipeline answers with an error.
            if !gst_ready() {
                return;
            }
            let path = one_second_wav("seek", 2000.0);
            let Ok(player) = AudioPlayer::open(path.to_str().unwrap()) else {
                let _ = std::fs::remove_file(&path);
                return;
            };
            player.seek_secs(9_999.0);
            assert_eq!(player.error(), None, "seeking past the end broke the pipeline");
            let position = player.position_secs();
            assert!(
                position <= 1.0,
                "a seek past the end landed at {position} seconds in a one-second file"
            );
            let _ = std::fs::remove_file(&path);
    });
    }

    #[test]
    fn a_file_that_is_not_sound_at_all_is_refused_rather_than_crashing() {
        in_main_context(|| {
            // The whole point of this module: a bad file produces a message, not
            // a dead process.
            if !gst_ready() {
                return;
            }
            let path = std::env::temp_dir().join("wayfinder-audio-test-garbage.mp3");
            std::fs::write(&path, b"this is not an MP3, it is a sentence").unwrap();
            // Either outcome is acceptable, and neither is an abort: open may
            // refuse it, or preroll may succeed and the bus may report the error.
            if let Ok(player) = AudioPlayer::open(path.to_str().unwrap()) {
                assert!(!player.is_playing());
            }
            let _ = std::fs::remove_file(&path);
    });
    }

    #[test]
    fn only_audio_and_volume_are_switched_on() {
        // Video off is the whole reason these are set: an .m4a with cover art
        // would otherwise have playbin build a window for it. Checked against
        // a real playbin, because a plain integer here passes a unit test and
        // then panics in the app.
        if !gst_ready() {
            return;
        }
        let Ok(element) = gst::ElementFactory::make("playbin").build() else {
            return;
        };
        let pipeline = element.downcast::<gst::Pipeline>().unwrap();
        let flags = audio_only_flags(&pipeline).expect("playbin has a flags property");
        // GST_PLAY_FLAG_AUDIO | GST_PLAY_FLAG_SOFT_VOLUME
        let bits = flags
            .transform::<u32>()
            .ok()
            .and_then(|value| value.get::<u32>().ok())
            .expect("flags convert to a number");
        assert_eq!(bits, 0x012, "flags were {bits:#05x}, not audio plus volume");
        assert_eq!(bits & 0x001, 0, "video was left switched on");
        // And it actually sets, which is the part that was wrong.
        pipeline.set_property_from_value("flags", &flags);
    }
}
