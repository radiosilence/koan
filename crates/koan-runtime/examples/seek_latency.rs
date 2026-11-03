//! How long a seek takes from the call to the Playhead slice that answers
//! it, stage by stage, and what the state stream says at rest.
//!
//! Plays a generated minute of silence through the real engine and output
//! against a throwaway configuration (`KOAN_CONFIG_DIR`, muted, no library
//! folders), then seeks it, paused and playing. For each seek it notes when
//! the call returned, when the player took it (`KoanEngine::seeks` moving,
//! read by a thread that does nothing else), when the seek's future
//! resolved, and when the first Playhead slice past the count arrived on
//! the stream. Every Playhead slice that arrives after a seek is asked is
//! checked: none past the count may read anywhere but the target.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_lite::future::block_on;
use koan_runtime::{KoanEngine, StateSlice};

const TRACK_SECS: u32 = 60;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        std::env::var_os("KOAN_CONFIG_DIR").ok_or("set KOAN_CONFIG_DIR to a throwaway one")?;
    let dir = std::path::PathBuf::from(dir);
    let config = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
    if !config.contains("muted = true") || !config.contains("folders = []") {
        return Err("the configuration must be muted and hold no library folders".into());
    }
    let wav = dir.join("silence.wav");
    write_wav(&wav, 44_100, TRACK_SECS)?;

    let engine = block_on(KoanEngine::open(None))?;
    let ids = block_on(engine.clone().import_paths(vec![wav]))?.track_ids;
    block_on(engine.clone().replace_queue(ids, None, false))?;

    // Every slice as it arrives, stamped.
    let (tx, rx) = crossbeam_channel::unbounded::<(Instant, StateSlice)>();
    let stream = engine.observe();
    std::thread::spawn(move || {
        while let Some(batch) = block_on(stream.next()) {
            let at = Instant::now();
            for slice in batch {
                if tx.send((at, slice)).is_err() {
                    return;
                }
            }
        }
    });
    let wait_playing = |rx: &crossbeam_channel::Receiver<(Instant, StateSlice)>| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok((_, StateSlice::Playhead { playing: true, .. })) =
                rx.recv_timeout(Duration::from_millis(100))
            {
                return true;
            }
        }
        false
    };
    if !wait_playing(&rx) {
        return Err("never started playing".into());
    }

    // At rest: what arrives in three quiet seconds, playing and paused.
    let rest = |rx: &crossbeam_channel::Receiver<(Instant, StateSlice)>, what: &str| {
        std::thread::sleep(Duration::from_millis(500));
        while rx.try_recv().is_ok() {}
        let end = Instant::now() + Duration::from_secs(3);
        let (mut playheads, mut playbacks, mut others) = (0, 0, 0);
        while let Ok((_, slice)) = rx.recv_deadline(end) {
            match slice {
                StateSlice::Playhead { .. } => playheads += 1,
                StateSlice::Playback { .. } => playbacks += 1,
                _ => others += 1,
            }
        }
        println!(
            "at rest, {what}: {:.1} Playhead/s, {:.1} Playback/s, {:.1} other/s",
            playheads as f64 / 3.0,
            playbacks as f64 / 3.0,
            others as f64 / 3.0
        );
    };
    rest(&rx, "playing");
    block_on(engine.clone().pause())?;
    rest(&rx, "paused");

    for playing in [false, true] {
        if playing {
            block_on(engine.clone().resume())?;
            wait_playing(&rx);
        }
        std::thread::sleep(Duration::from_millis(300));
        let mut rows = Vec::new();
        let mut stale = 0;
        let mut published = 0;
        for i in 0..20u64 {
            while rx.try_recv().is_ok() {}
            let target = 5_000 + (i % 5) * 9_000 + if i % 2 == 0 { 0 } else { 30_000 };
            let before = engine.seeks();
            // The take, seen by a thread that only watches for it.
            let watching = Arc::new(AtomicBool::new(true));
            let taker = {
                let engine = engine.clone();
                let watching = watching.clone();
                std::thread::spawn(move || {
                    while watching.load(Ordering::Relaxed) {
                        if engine.seeks() > before {
                            return Some(Instant::now());
                        }
                        std::hint::spin_loop();
                    }
                    None
                })
            };
            let start = Instant::now();
            let future = engine.clone().seek(target);
            let called = Instant::now();
            block_on(future)?;
            let resolved = Instant::now();
            let mut answered = None;
            let deadline = Instant::now() + Duration::from_secs(2);
            while let Ok((at, slice)) = rx.recv_deadline(deadline) {
                let StateSlice::Playhead {
                    position_ms, seek, ..
                } = slice
                else {
                    continue;
                };
                published += 1;
                if seek <= before {
                    continue;
                }
                // Playing, it may have moved on by the time it was read.
                let slack = if playing { 100 } else { 0 };
                if position_ms < target || position_ms > target + slack {
                    stale += 1;
                }
                if answered.is_none() {
                    answered = Some(at);
                }
                if answered.is_some_and(|a| at.duration_since(a) > Duration::from_millis(300)) {
                    break;
                }
            }
            watching.store(false, Ordering::Relaxed);
            let taken = taker.join().unwrap();
            let answered = answered.ok_or("a seek was never answered")?;
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            rows.push([
                ms(called - start),
                taken.map_or(f64::NAN, |t| ms(t - start)),
                ms(resolved - start),
                ms(answered - start),
            ]);
        }
        let stage = |i: usize| {
            let mut v: Vec<f64> = rows
                .iter()
                .map(|r| r[i])
                .filter(|x| x.is_finite())
                .collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (v[v.len() / 2], v[v.len() - 1])
        };
        println!(
            "seeks, {}: {} Playhead slices, {} past the count away from the target",
            if playing { "playing" } else { "paused" },
            published,
            stale
        );
        for (i, name) in [
            "call returned",
            "player took it",
            "future resolved",
            "answering Playhead read",
        ]
        .iter()
        .enumerate()
        {
            let (median, max) = stage(i);
            println!("  {name:>24}: median {median:.3} ms, max {max:.3} ms");
        }
    }
    block_on(engine.clone().stop())?;
    Ok(())
}

/// A minute of 16-bit stereo silence.
fn write_wav(path: &std::path::Path, rate: u32, secs: u32) -> std::io::Result<()> {
    use std::io::Write;
    let data = rate * secs * 4;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    let zeros = vec![0u8; 1 << 16];
    let mut left = data as usize;
    while left > 0 {
        let n = left.min(zeros.len());
        f.write_all(&zeros[..n])?;
        left -= n;
    }
    f.flush()
}
