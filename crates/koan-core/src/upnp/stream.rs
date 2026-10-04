//! A stream koan processed itself, served to a renderer.
//!
//! When the output's profile does anything, the renderer cannot be handed the
//! original file: what the profile does to the samples would never reach it.
//! The session is decoded and processed here as it would be for this device,
//! into the same ring, and the encoder drains the ring instead of an audio
//! callback. Its output is one endless FLAC (or WAV) stream per session,
//! served on a token like a file. Track changes inside the session are the
//! timeline's boundaries; the renderer is never asked to switch.
//!
//! The renderer paces the stream. Encoded bytes queue in a `Pipe` of bounded
//! size, so a renderer that stops reading (paused, or its buffer full) holds
//! the encoder, which holds the ring, which holds the decoder.
//!
//! Renderers often open a URL more than once before playing it: a probe that
//! reads the first kilobytes and hangs up, then the real fetch. Until
//! `EARLY` bytes have gone out, a new connection is served the stream from
//! its start. After that a new connection joins where the stream is, and
//! `Pipe::origin_ms` says how far into the stream that was, which is what the
//! renderer's position counts from.

use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use flacenc::component::{BitRepr, Stream, StreamInfo};
use flacenc::error::Verify;
use flacenc::source::{Fill, FrameBuf};
use parking_lot::{Condvar, Mutex};

use super::serve::Request;
use crate::audio::buffer::PlaybackTimeline;
use crate::player::state::QueueItemId;

/// Frames per FLAC block, and per chunk of the pipe.
const BLOCK: usize = 4096;

/// How many encoded bytes wait for the renderer before the encoder stops.
const QUEUED: usize = 512 * 1024;

/// Bytes served before a reconnect joins the stream where it is rather than
/// at its start.
const EARLY: usize = 1024 * 1024;

/// Bytes of audio between two ICY metadata blocks.
const ICY_INTERVAL: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Flac,
    Wav,
}

impl Encoding {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }
}

/// What the stream carries: the session's output rate and channels, and the
/// integer depth the samples are dithered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub encoding: Encoding,
    pub rate: u32,
    pub channels: u16,
    pub bits: u8,
}

impl Format {
    /// The depth a source's samples are sent at: its own, within what both
    /// encodings carry, and 24 for a source that has none (a lossy one).
    pub fn bits_for(source: Option<u16>) -> u8 {
        source.map_or(24, |b| b.clamp(16, 24) as u8)
    }
}

struct Chunk {
    bytes: Vec<u8>,
    /// Frames of the stream before this chunk's first.
    start: u64,
    frames: u64,
    /// The track playing at this chunk's first frame.
    track: Option<QueueItemId>,
}

#[derive(Default)]
struct State {
    header: Vec<u8>,
    queue: VecDeque<Arc<Chunk>>,
    queued: usize,
    /// Every chunk served, while fewer than `EARLY` bytes have been.
    history: Option<Vec<Arc<Chunk>>>,
    served: usize,
    /// The first frame of the next chunk a connection will be served.
    next_frame: u64,
    /// The newest connection, the only one served.
    connection: u64,
    finished: bool,
    closed: bool,
}

/// Between the encoder and whichever connection the renderer has open.
pub struct Pipe {
    state: Mutex<State>,
    changed: Condvar,
    rate: u32,
    origin: AtomicU64,
    mime: String,
    title: Box<dyn Fn(QueueItemId) -> String + Send + Sync>,
}

impl Pipe {
    pub fn new(
        format: Format,
        mime: &str,
        title: impl Fn(QueueItemId) -> String + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                history: Some(Vec::new()),
                ..Default::default()
            }),
            changed: Condvar::new(),
            rate: format.rate,
            origin: AtomicU64::new(0),
            mime: mime.to_string(),
            title: Box::new(title),
        })
    }

    /// Where in the stream the renderer's connection began, which its
    /// position counts from.
    pub fn origin_ms(&self) -> u64 {
        self.origin.load(Ordering::Acquire) * 1000 / self.rate.max(1) as u64
    }

    /// End the stream now: the encoder and every connection let go.
    pub fn close(&self) {
        self.state.lock().closed = true;
        self.changed.notify_all();
    }

    fn set_header(&self, header: Vec<u8>) {
        self.state.lock().header = header;
    }

    /// Queue a chunk, waiting while the renderer has enough. False once the
    /// pipe is closed.
    fn push(&self, chunk: Chunk) -> bool {
        let mut state = self.state.lock();
        while state.queued >= QUEUED && !state.closed {
            self.changed.wait(&mut state);
        }
        if state.closed {
            return false;
        }
        log::debug!(
            "upnp: chunk at frame {} queued behind {} bytes",
            chunk.start,
            state.queued
        );
        state.queued += chunk.bytes.len();
        state.queue.push_back(Arc::new(chunk));
        drop(state);
        self.changed.notify_all();
        true
    }

    /// Everything has been queued: a connection that drains the queue ends.
    fn finish(&self) {
        self.state.lock().finished = true;
        self.changed.notify_all();
    }

    fn connect(&self) -> Reader {
        let mut state = self.state.lock();
        state.connection += 1;
        let (replay, origin) = match &state.history {
            Some(history) => (history.iter().cloned().collect(), 0),
            None => (
                VecDeque::new(),
                state.queue.front().map_or(state.next_frame, |c| c.start),
            ),
        };
        self.origin.store(origin, Ordering::Release);
        let connection = state.connection;
        drop(state);
        // A connection it replaces is waiting on the queue, and has to see
        // that it is no longer served.
        self.changed.notify_all();
        Reader { connection, replay }
    }

    /// The next chunk for `reader`, waiting for the encoder. `None` when the
    /// stream is over for it: finished, closed, or another connection opened.
    fn next(&self, reader: &mut Reader) -> Option<Arc<Chunk>> {
        if let Some(chunk) = reader.replay.pop_front() {
            return Some(chunk);
        }
        let mut state = self.state.lock();
        loop {
            if state.closed || state.connection != reader.connection {
                return None;
            }
            if let Some(chunk) = state.queue.pop_front() {
                state.queued -= chunk.bytes.len();
                state.served += chunk.bytes.len();
                state.next_frame = chunk.start + chunk.frames;
                if state.served > EARLY {
                    state.history = None;
                } else if let Some(history) = state.history.as_mut() {
                    history.push(chunk.clone());
                }
                drop(state);
                self.changed.notify_all();
                return Some(chunk);
            }
            if state.finished {
                return None;
            }
            self.changed.wait(&mut state);
        }
    }
}

struct Reader {
    connection: u64,
    replay: VecDeque<Arc<Chunk>>,
}

/// Serve the stream to one connection, for as long as it is the renderer's.
pub(crate) fn serve(stream: &mut TcpStream, req: &Request, pipe: &Pipe) -> std::io::Result<()> {
    let icy = req.header("Icy-MetaData").is_some_and(|v| v.trim() == "1");
    // No length and no ranges: the stream is made as it is sent. CI=1 says
    // it is converted from the original.
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: none\r\n{}transferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: DLNA.ORG_OP=00;DLNA.ORG_CI=1;DLNA.ORG_FLAGS=01700000000000000000000000000000\r\nConnection: close\r\n\r\n",
        pipe.mime,
        if icy {
            format!("icy-metaint: {ICY_INTERVAL}\r\n")
        } else {
            String::new()
        }
    )?;
    if req.method == "HEAD" {
        return Ok(());
    }
    stream.set_write_timeout(None)?;
    let header = pipe.state.lock().header.clone();
    let mut out = Icy {
        stream,
        every: icy.then_some(ICY_INTERVAL),
        left: ICY_INTERVAL,
        title: String::new(),
        sent: String::new(),
    };
    let mut reader = pipe.connect();
    log::info!(
        "upnp: stream connection {} ({}{}), from {}ms",
        reader.connection,
        req.header("User-Agent").unwrap_or("no agent"),
        req.header("Range")
            .map(|r| format!(", {r}"))
            .unwrap_or_default(),
        pipe.origin_ms()
    );
    let mut sent = 0usize;
    let result = (|| {
        out.write(&header)?;
        while let Some(chunk) = pipe.next(&mut reader) {
            if let Some(track) = chunk.track {
                out.title = (pipe.title)(track);
            }
            out.write(&chunk.bytes)?;
            sent += chunk.bytes.len();
        }
        Ok::<_, std::io::Error>(())
    })();
    log::info!(
        "upnp: stream connection {} ended after {sent} bytes: {}",
        reader.connection,
        match &result {
            Err(e) => format!("the renderer hung up ({e})"),
            Ok(()) => "the stream ended or another connection replaced it".into(),
        }
    );
    result
}

/// Audio bytes with ICY metadata blocks between them, when the renderer asked
/// for them: the title of the track at that point in the stream, sent when it
/// changes.
struct Icy<'a> {
    stream: &'a mut TcpStream,
    every: Option<usize>,
    left: usize,
    title: String,
    sent: String,
}

impl Icy<'_> {
    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<()> {
        let Some(every) = self.every else {
            return self.stream.write_all(bytes);
        };
        while !bytes.is_empty() {
            let n = self.left.min(bytes.len());
            self.stream.write_all(&bytes[..n])?;
            bytes = &bytes[n..];
            self.left -= n;
            if self.left == 0 {
                let block = if self.title == self.sent {
                    vec![0]
                } else {
                    self.sent = self.title.clone();
                    icy_block(&self.title)
                };
                self.stream.write_all(&block)?;
                self.left = every;
            }
        }
        Ok(())
    }
}

/// One ICY metadata block: its length in sixteens, then `StreamTitle`, padded.
fn icy_block(title: &str) -> Vec<u8> {
    let title = title.replace('\'', "’");
    let mut text = format!("StreamTitle='{title}';").into_bytes();
    text.truncate(255 * 16);
    let sixteens = text.len().div_ceil(16);
    text.resize(sixteens * 16, 0);
    let mut block = vec![sixteens as u8];
    block.extend(text);
    block
}

/// The encoder thread. Dropping it waits for the thread, so close the pipe
/// first.
pub struct Encoder {
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Drain `consumer` into `pipe`, encoded as `format` says, until the decoder
/// lets go of the ring or the pipe is closed. `decoder` is woken whenever the
/// ring has room: a decoder parked on a full ring waits for as long as half
/// of it takes to play, and the renderer reads faster than that at first.
pub fn start(
    consumer: rtrb::Consumer<f32>,
    format: Format,
    pipe: Arc<Pipe>,
    timeline: Arc<PlaybackTimeline>,
    decoder: Option<thread::Thread>,
) -> std::io::Result<Encoder> {
    let mut codec = Codec::new(format).map_err(std::io::Error::other)?;
    pipe.set_header(codec.header());
    let thread = thread::Builder::new()
        .name("koan-upnp-encode".into())
        .spawn(move || {
            encode(
                consumer,
                format,
                &mut codec,
                &pipe,
                &timeline,
                decoder.as_ref(),
            )
        })?;
    Ok(Encoder {
        thread: Some(thread),
    })
}

fn encode(
    mut consumer: rtrb::Consumer<f32>,
    format: Format,
    codec: &mut Codec,
    pipe: &Pipe,
    timeline: &PlaybackTimeline,
    decoder: Option<&thread::Thread>,
) {
    let channels = format.channels as usize;
    let block = BLOCK * channels;
    let mut samples: Vec<f32> = Vec::with_capacity(block);
    let mut ints: Vec<i32> = Vec::with_capacity(block);
    let mut dither = Dither::new(format.bits);
    let mut frames: u64 = 0;
    loop {
        let wanted = block - samples.len();
        let ready = consumer.slots().min(wanted);
        if ready > 0
            && let Ok(chunk) = consumer.read_chunk(ready)
        {
            samples.extend(chunk);
            if let Some(decoder) = decoder {
                decoder.unpark();
            }
        }
        let ended = consumer.is_abandoned() && consumer.is_empty();
        if samples.len() == block || ended && !samples.is_empty() {
            ints.clear();
            ints.extend(samples.iter().map(|&s| dither.quantise(s)));
            let bytes = codec.encode(&ints, samples.len() / channels);
            let track = timeline.track_at(frames * channels as u64);
            if !pipe.push(Chunk {
                bytes,
                start: frames,
                frames: (samples.len() / channels) as u64,
                track,
            }) {
                return;
            }
            frames += (samples.len() / channels) as u64;
            samples.clear();
            continue;
        }
        if ended {
            pipe.finish();
            return;
        }
        if pipe.state.lock().closed {
            return;
        }
        if ready == 0 {
            log::debug!("upnp: encoder waiting on an empty ring at frame {frames}");
        }
        // An empty ring is the start of a session or its end; while it plays,
        // the renderer's reading is what this thread waits on, in `push`.
        if ready == 0 {
            thread::park_timeout(Duration::from_millis(10));
        }
    }
}

/// f32 to integers at `bits`, with triangular dither of one LSB either way:
/// truncating without it would add distortion the chain did not ask for.
struct Dither {
    scale: f64,
    rng: u64,
}

impl Dither {
    fn new(bits: u8) -> Self {
        Self {
            scale: (1u64 << (bits - 1)) as f64,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn uniform(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn quantise(&mut self, sample: f32) -> i32 {
        let noise = self.uniform() - self.uniform();
        (sample as f64 * self.scale + noise)
            .round()
            .clamp(-self.scale, self.scale - 1.0) as i32
    }
}

enum Codec {
    Flac {
        config: Box<flacenc::error::Verified<flacenc::config::Encoder>>,
        info: StreamInfo,
        buffer: FrameBuf,
        frame: usize,
    },
    Wav {
        format: Format,
    },
}

impl Codec {
    fn new(format: Format) -> Result<Self, String> {
        Ok(match format.encoding {
            Encoding::Flac => {
                let mut info = StreamInfo::new(
                    format.rate as usize,
                    format.channels as usize,
                    format.bits as usize,
                )
                .map_err(|e| e.to_string())?;
                info.set_block_sizes(BLOCK, BLOCK)
                    .map_err(|e| e.to_string())?;
                Self::Flac {
                    config: Box::new(
                        flacenc::config::Encoder::default()
                            .into_verified()
                            .map_err(|(_, e)| e.to_string())?,
                    ),
                    info,
                    buffer: FrameBuf::with_size(format.channels as usize, BLOCK)
                        .map_err(|e| e.to_string())?,
                    frame: 0,
                }
            }
            Encoding::Wav => Self::Wav { format },
        })
    }

    /// The start of the stream. No length is known, so a FLAC stream says
    /// zero samples, as the format allows, and a WAV the largest size.
    fn header(&self) -> Vec<u8> {
        match self {
            Self::Flac { info, .. } => {
                let mut sink = flacenc::bitsink::MemSink::<u8>::new();
                Stream::with_stream_info(info.clone())
                    .write(&mut sink)
                    .expect("writing to memory");
                sink.as_slice().to_vec()
            }
            Self::Wav { format } => {
                let bytes = format.bits as u32 / 8;
                let align = bytes * format.channels as u32;
                let mut h = Vec::with_capacity(44);
                h.extend_from_slice(b"RIFF");
                h.extend_from_slice(&u32::MAX.to_le_bytes());
                h.extend_from_slice(b"WAVEfmt ");
                h.extend_from_slice(&16u32.to_le_bytes());
                h.extend_from_slice(&1u16.to_le_bytes());
                h.extend_from_slice(&format.channels.to_le_bytes());
                h.extend_from_slice(&format.rate.to_le_bytes());
                h.extend_from_slice(&(format.rate * align).to_le_bytes());
                h.extend_from_slice(&(align as u16).to_le_bytes());
                h.extend_from_slice(&(format.bits as u16).to_le_bytes());
                h.extend_from_slice(b"data");
                h.extend_from_slice(&u32::MAX.to_le_bytes());
                h
            }
        }
    }

    fn encode(&mut self, ints: &[i32], frames: usize) -> Vec<u8> {
        match self {
            Self::Flac {
                config,
                info,
                buffer,
                frame,
            } => {
                if buffer.size() != frames {
                    buffer.resize(frames);
                }
                buffer.fill_interleaved(ints).expect("samples in range");
                let encoded = flacenc::encode_fixed_size_frame(config, buffer, *frame, info)
                    .expect("frame number in range");
                *frame += 1;
                let mut sink = flacenc::bitsink::MemSink::<u8>::new();
                encoded.write(&mut sink).expect("writing to memory");
                sink.as_slice().to_vec()
            }
            Self::Wav { format } => {
                let bytes = format.bits as usize / 8;
                let mut out = Vec::with_capacity(ints.len() * bytes);
                for s in ints {
                    out.extend_from_slice(&s.to_le_bytes()[..bytes]);
                }
                out
            }
        }
    }
}

/// Decode a whole stream, for tests that check what a renderer was sent.
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8], extension: &str) -> (u32, Vec<f32>) {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    let mss = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .unwrap();
    let track = reader.default_track(TrackType::Audio).unwrap();
    let id = track.id;
    let params = track.codec_params.as_ref().unwrap().audio().unwrap();
    let rate = params.sample_rate.unwrap();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .unwrap();
    let mut out = Vec::new();
    while let Ok(Some(packet)) = reader.next_packet() {
        if packet.track_id != id {
            continue;
        }
        let decoded = decoder.decode(&packet).unwrap();
        let mut samples = vec![0f32; decoded.samples_interleaved()];
        decoded.copy_to_slice_interleaved(&mut samples);
        out.extend(samples);
    }
    (rate, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stereo sine through the encoder, as the renderer would read it.
    fn round_trip(encoding: Encoding, bits: u8) -> (Vec<f32>, Vec<f32>, u32) {
        let format = Format {
            encoding,
            rate: 48_000,
            channels: 2,
            bits,
        };
        let input: Vec<f32> = (0..30_000)
            .flat_map(|i| {
                let s = (i as f32 * 0.031).sin() * 0.5;
                [s, -s]
            })
            .collect();
        let (mut producer, consumer) = rtrb::RingBuffer::new(input.len());
        for s in &input {
            producer.push(*s).unwrap();
        }
        drop(producer);
        let pipe = Pipe::new(format, "audio/flac", |_| String::new());
        let encoder = start(
            consumer,
            format,
            pipe.clone(),
            PlaybackTimeline::new(),
            None,
        )
        .unwrap();
        let mut bytes = pipe.state.lock().header.clone();
        let mut reader = pipe.connect();
        while let Some(chunk) = pipe.next(&mut reader) {
            bytes.extend_from_slice(&chunk.bytes);
        }
        drop(encoder);
        let (rate, output) = decode(&bytes, encoding.extension());
        (input, output, rate)
    }

    #[test]
    fn a_flac_stream_decodes_to_what_was_sent_within_the_dither() {
        let (input, output, rate) = round_trip(Encoding::Flac, 24);
        assert_eq!(rate, 48_000);
        assert_eq!(output.len(), input.len());
        let lsb = 1.0 / (1u32 << 23) as f32;
        let worst = input
            .iter()
            .zip(&output)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(worst <= 2.0 * lsb, "worst error {worst}, lsb {lsb}");
    }

    #[test]
    fn a_wav_stream_at_sixteen_bits_decodes_within_the_dither() {
        let (input, output, _) = round_trip(Encoding::Wav, 16);
        assert_eq!(output.len(), input.len());
        let lsb = 1.0 / 32768.0;
        for (a, b) in input.iter().zip(&output) {
            assert!((a - b).abs() <= 2.0 * lsb, "{a} vs {b}");
        }
    }

    #[test]
    fn dither_is_unbiased_and_one_lsb_wide() {
        let mut dither = Dither::new(16);
        let target = 0.25 / 32768.0;
        let n = 200_000;
        let values: Vec<i32> = (0..n).map(|_| dither.quantise(target)).collect();
        assert!(values.iter().all(|v| (-1..=1).contains(v)));
        let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        assert!((mean - 0.25).abs() < 0.01, "mean {mean}");
    }

    #[test]
    fn icy_blocks_are_padded_sixteens() {
        let block = icy_block("Polar Bear – Peepers");
        assert_eq!(block.len(), 1 + block[0] as usize * 16);
        assert!(block[1..].starts_with(b"StreamTitle='Polar Bear"));
        assert_eq!(icy_block("it's").len() % 16, 1);
    }

    /// Kodi's pattern: a probe that hangs up, a second connection, and a
    /// third opened while the second is still reading, which is the one it
    /// plays. The third must go on getting the stream after the replay.
    #[test]
    fn the_connection_that_replaces_an_open_one_is_fed_past_the_replay() {
        use std::io::Read;
        let format = Format {
            encoding: Encoding::Wav,
            rate: 44_100,
            channels: 2,
            bits: 16,
        };
        let (mut producer, consumer) = rtrb::RingBuffer::new(1 << 16);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let feeding = stop.clone();
        let feeder = thread::spawn(move || {
            while !feeding.load(Ordering::Relaxed) {
                if producer.push(0.25).is_err() {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        });
        let pipe = Pipe::new(format, "audio/wav", |_| String::new());
        let _encoder = start(
            consumer,
            format,
            pipe.clone(),
            PlaybackTimeline::new(),
            None,
        )
        .unwrap();
        let listener = super::super::serve::Listener::start(Box::new(|_, _| {})).unwrap();
        let token = listener.add(super::super::serve::Served::Stream {
            pipe: pipe.clone(),
            art: Default::default(),
        });
        let open = || {
            let mut s = TcpStream::connect(("127.0.0.1", listener.port())).unwrap();
            write!(s, "GET /t/{token}.wav HTTP/1.1\r\nRange: bytes=0-\r\n\r\n").unwrap();
            s
        };
        let read = |s: &mut TcpStream, n: usize| {
            let mut buf = vec![0u8; n];
            s.read_exact(&mut buf).unwrap();
        };
        let mut probe = open();
        read(&mut probe, 160_000);
        drop(probe);
        let mut second = open();
        read(&mut second, 340_000);
        let mut third = open();
        // The replay, and well past it.
        read(&mut third, 2_000_000);
        drop(second);
        stop.store(true, Ordering::Relaxed);
        pipe.close();
        feeder.join().unwrap();
    }

    #[test]
    fn a_reconnect_early_replays_from_the_start_and_later_joins_live() {
        let format = Format {
            encoding: Encoding::Wav,
            rate: 1000,
            channels: 1,
            bits: 16,
        };
        let pipe = Pipe::new(format, "audio/wav", |_| String::new());
        let chunk = |start| Chunk {
            bytes: vec![0; 400 * 1024],
            start,
            frames: 1000,
            track: None,
        };
        pipe.push(chunk(0));
        pipe.push(chunk(1000));
        let mut probe = pipe.connect();
        assert_eq!(pipe.next(&mut probe).unwrap().start, 0);
        // A second connection replaces the probe and starts again.
        let mut real = pipe.connect();
        assert!(pipe.next(&mut probe).is_none());
        assert_eq!(pipe.next(&mut real).unwrap().start, 0);
        assert_eq!(pipe.next(&mut real).unwrap().start, 1000);
        assert_eq!(pipe.origin_ms(), 0);
        // Past `EARLY`, a reconnect joins at the next chunk.
        pipe.push(chunk(2000));
        pipe.push(chunk(3000));
        assert_eq!(pipe.next(&mut real).unwrap().start, 2000);
        let mut again = pipe.connect();
        assert_eq!(pipe.origin_ms(), 3000);
        assert_eq!(pipe.next(&mut again).unwrap().start, 3000);
    }
}
