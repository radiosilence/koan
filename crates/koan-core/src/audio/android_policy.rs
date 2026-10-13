//! The decisions behind the Android backend, apart from the calls that carry
//! them out.
//!
//! `android_backend.rs` is AAudio and nothing else: it cannot be built, let
//! alone run, off a phone. What it should do with a stream (how to open one,
//! when a failure means the route moved, which rate to report) needs no phone,
//! so it is here as plain functions that the tests on a desktop exercise. The
//! data callback's body is here too, for the same reason: it is a function of
//! the ring and some atomics, and the rule that it touches nothing else is
//! easier to hold in a file that does not name AAudio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use super::fade::Fader;

/// How the output is shared with the rest of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// Direct to the device's own buffer, where the device allows it. The
    /// only mode in which the rate asked for can be the rate the hardware
    /// runs at.
    Exclusive,
    /// Through the system mixer, which resamples to its own rate.
    Shared,
}

/// One way of opening the stream, tried in the order `open_attempts` gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    pub sharing: Sharing,
    /// The rate asked for, in Hz.
    pub sample_rate: u32,
}

/// The ways to open a stream at `sample_rate`, best first: exclusive, then
/// shared.
///
/// Every attempt asks for the source's own rate and none asks for "whatever
/// the device prefers". The ring holds samples at the source rate, so a
/// stream at any other rate would play them at the wrong speed; a route that
/// cannot run the rate itself has the system mixer convert it instead, which
/// is what a shared stream is.
pub fn open_attempts(sample_rate: u32) -> [Attempt; 2] {
    [
        Attempt {
            sharing: Sharing::Exclusive,
            sample_rate,
        },
        Attempt {
            sharing: Sharing::Shared,
            sample_rate,
        },
    ]
}

/// Whether a stream that opened is one to keep: the rate and channel count it
/// reports are the ones asked for. A stream that opened at another rate would
/// play the ring at the wrong speed, so the next attempt is tried instead.
pub fn keeps(attempt: Attempt, granted_rate: i32, channels: i32, granted_channels: i32) -> bool {
    granted_rate > 0 && granted_rate as u32 == attempt.sample_rate && channels == granted_channels
}

/// What went wrong with a stream, as far as deciding what to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFault {
    /// The device the stream was on has gone, or the system moved the stream
    /// off it: headphones unplugged, Bluetooth connected or lost, a USB DAC
    /// attached or pulled. AAudio reports this through the error callback.
    Disconnected,
    /// Anything else. The stream may still be usable, and a reopen would not
    /// obviously help.
    Other,
}

/// Whether `fault` means the route changed, so the stream is to be reopened
/// on whatever the system now routes to.
pub fn reopens(fault: StreamFault) -> bool {
    fault == StreamFault::Disconnected
}

/// How long to wait before each try at reopening after a disconnect.
///
/// The first is immediate. When a USB device is pulled or Bluetooth drops,
/// the system can still be settling on the next route, and an open then can
/// fail or land on a route that is about to move again; the later tries wait
/// it out. After the last one the output is given up on until the player next
/// starts it.
pub const REOPEN_DELAYS: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_millis(150),
    Duration::from_millis(400),
    Duration::from_millis(1000),
    Duration::from_millis(2500),
];

/// What a stream was granted, from what the system reports after opening it
/// rather than what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Granted {
    pub sharing: Sharing,
    pub sample_rate: u32,
    /// The device the stream is on; 0 if the system did not say.
    pub device_id: i32,
}

/// The rate the output hardware runs at, to report beside the source's.
///
/// An exclusive stream is the hardware's own, so it runs at the rate it was
/// granted. A shared one is converted by the mixer to the rate the route
/// runs at, which AAudio does not report: the app reads it from the platform
/// (`route_rate`, 0 while unknown). Where it is not known, the stream's rate
/// is the best there is to say.
pub fn hardware_rate(granted: Granted, route_rate: u32) -> u32 {
    match granted.sharing {
        Sharing::Exclusive => granted.sample_rate,
        Sharing::Shared if route_rate > 0 => route_rate,
        Sharing::Shared => granted.sample_rate,
    }
}

/// The rate to answer when asked to switch the device to `wanted`.
///
/// There is no nominal rate to set on Android: the stream is opened at the
/// source's and the system either honours it or converts. Until a stream is
/// open all there is to go on is the route's reported rate, which a shared
/// stream will be converted to; with none known, `wanted` stands in, and the
/// rate the stream is granted corrects it.
pub fn expected_rate(wanted: f64, route_rate: u32) -> f64 {
    if route_rate > 0 {
        route_rate as f64
    } else {
        wanted
    }
}

/// Rates a route is listed as taking. A shared stream takes any, since the
/// mixer converts, so this is the usual set rather than a probe.
pub const LISTED_RATES: [f64; 6] = [44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0];

/// Everything the data callback touches: the ring's consumer, the fader and
/// the play-position counter.
pub struct Output {
    pub consumer: rtrb::Consumer<f32>,
    pub fader: Fader,
    pub samples_played: Arc<AtomicU64>,
    pub channels: usize,
}

/// The data callback's body. Atomics and the ring only: no allocation, no
/// lock, no logging, no JNI.
///
/// `data` is `frames` interleaved f32 frames of `output.channels` channels.
pub fn render(output: &mut Output, running: &AtomicBool, data: &mut [f32]) {
    if !running.load(Ordering::Acquire) {
        data.fill(0.0);
        return;
    }

    let total = data.len();
    let channels = output.channels;
    let available = output.consumer.slots();
    let to_read = available.min(output.fader.readable(total, channels));

    if to_read > 0
        && let Ok(chunk) = output.consumer.read_chunk(to_read)
    {
        let (first, second) = chunk.as_slices();
        let copy_total = (first.len() + second.len()).min(total);
        let first_copy = first.len().min(copy_total);
        let second_copy = (copy_total - first_copy).min(second.len());
        data[..first_copy].copy_from_slice(&first[..first_copy]);
        data[first_copy..first_copy + second_copy].copy_from_slice(&second[..second_copy]);
        chunk.commit_all();
        output
            .samples_played
            .fetch_add(copy_total as u64, Ordering::AcqRel);
        output.fader.apply(&mut data[..copy_total], channels);
    }

    // Zero-pad the remainder on underrun: silence over glitches.
    if to_read < total {
        data[to_read..].fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_is_tried_before_shared_and_both_at_the_source_rate() {
        let attempts = open_attempts(96000);
        assert_eq!(attempts[0].sharing, Sharing::Exclusive);
        assert_eq!(attempts[1].sharing, Sharing::Shared);
        assert!(attempts.iter().all(|a| a.sample_rate == 96000));
    }

    #[test]
    fn a_stream_at_another_rate_or_width_is_not_kept() {
        let attempt = open_attempts(44100)[1];
        assert!(keeps(attempt, 44100, 2, 2));
        assert!(!keeps(attempt, 48000, 2, 2));
        assert!(!keeps(attempt, 44100, 2, 1));
        assert!(!keeps(attempt, 0, 2, 2));
        assert!(!keeps(attempt, -1, 2, 2));
    }

    #[test]
    fn only_a_disconnect_reopens() {
        assert!(reopens(StreamFault::Disconnected));
        assert!(!reopens(StreamFault::Other));
    }

    #[test]
    fn reopening_starts_at_once_and_backs_off() {
        assert_eq!(REOPEN_DELAYS[0], Duration::ZERO);
        assert!(REOPEN_DELAYS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn exclusive_runs_at_the_rate_it_was_granted() {
        let granted = Granted {
            sharing: Sharing::Exclusive,
            sample_rate: 96000,
            device_id: 7,
        };
        assert_eq!(hardware_rate(granted, 48000), 96000);
        assert_eq!(hardware_rate(granted, 0), 96000);
    }

    #[test]
    fn shared_runs_at_the_routes_rate_when_it_is_known() {
        let granted = Granted {
            sharing: Sharing::Shared,
            sample_rate: 44100,
            device_id: 0,
        };
        assert_eq!(hardware_rate(granted, 48000), 48000);
        assert_eq!(hardware_rate(granted, 0), 44100);
    }

    #[test]
    fn the_rate_expected_before_a_stream_opens() {
        assert_eq!(expected_rate(96000.0, 48000), 48000.0);
        assert_eq!(expected_rate(96000.0, 0), 96000.0);
    }

    /// An output over a ring of `capacity` samples, with a producer to fill it.
    fn output(capacity: usize, channels: usize) -> (rtrb::Producer<f32>, Output, Arc<AtomicU64>) {
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        let played = Arc::new(AtomicU64::new(0));
        let output = Output {
            consumer,
            fader: Fader::new(crate::audio::fade::FadeControl::new(), 48000.0),
            samples_played: played.clone(),
            channels,
        };
        (producer, output, played)
    }

    fn push(producer: &mut rtrb::Producer<f32>, samples: impl IntoIterator<Item = f32>) {
        for s in samples {
            producer.push(s).unwrap();
        }
    }

    #[test]
    fn the_callback_plays_the_ring_untouched_and_counts_it() {
        let (mut producer, mut output, played) = output(64, 2);
        let running = AtomicBool::new(true);
        push(&mut producer, (0..8).map(|i| i as f32 * 0.125));
        let mut data = [9.0f32; 8];
        render(&mut output, &running, &mut data);
        let expected: Vec<f32> = (0..8).map(|i| i as f32 * 0.125).collect();
        assert_eq!(data.to_vec(), expected);
        assert_eq!(played.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn an_underrun_is_padded_with_silence_and_counts_only_what_played() {
        let (mut producer, mut output, played) = output(64, 2);
        let running = AtomicBool::new(true);
        push(&mut producer, [0.5, -0.5, 0.25, -0.25]);
        let mut data = [9.0f32; 8];
        render(&mut output, &running, &mut data);
        assert_eq!(data, [0.5, -0.5, 0.25, -0.25, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(played.load(Ordering::Relaxed), 4);
        // And an empty ring is all silence.
        render(&mut output, &running, &mut data);
        assert_eq!(data, [0.0; 8]);
        assert_eq!(played.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn a_callback_that_is_not_running_leaves_the_ring_alone() {
        let (mut producer, mut output, played) = output(64, 2);
        let running = AtomicBool::new(false);
        push(&mut producer, [1.0; 8]);
        let mut data = [9.0f32; 8];
        render(&mut output, &running, &mut data);
        assert_eq!(data, [0.0; 8]);
        assert_eq!(played.load(Ordering::Relaxed), 0);
        running.store(true, Ordering::Release);
        render(&mut output, &running, &mut data);
        assert_eq!(data, [1.0; 8]);
    }

    #[test]
    fn a_read_that_wraps_the_ring_keeps_its_order() {
        // Capacity 8: read 6, write 6 more, so the next read of 8 spans the
        // end of the buffer.
        let (mut producer, mut output, played) = output(8, 2);
        let running = AtomicBool::new(true);
        push(&mut producer, (0..6).map(|i| i as f32));
        let mut data = [0.0f32; 6];
        render(&mut output, &running, &mut data);
        push(&mut producer, (6..14).map(|i| i as f32));
        let mut data = [0.0f32; 8];
        render(&mut output, &running, &mut data);
        let expected: Vec<f32> = (6..14).map(|i| i as f32).collect();
        assert_eq!(data.to_vec(), expected);
        assert_eq!(played.load(Ordering::Relaxed), 14);
    }
}
