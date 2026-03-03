# DSP Pipeline & Downloadable Headphone Correction Profiles

Feasibility plan for adding a DSP processing stage and headphone/speaker correction profile support to koan.

## Current Audio Data Flow

```
File → Symphonia decode → SampleBuffer<f32> (interleaved)
     → rtrb::Producer::write_chunk_uninit()
     → Ring Buffer (SPSC, ~192k samples)
     → rtrb::Consumer::read_chunk()          ← CoreAudio RT thread
     → AudioBufferList → DAC
```

Key constraints:
- **CoreAudio render callback MUST NOT allocate or lock.** It only touches atomics + ring buffer consumer.
- Decode thread owns the producer side, sleeps 500us on backpressure.
- No resampling — device sample rate is switched to match source (bit-perfect).
- ReplayGain exists but is **not currently applied in the decode path** — `replaygain.rs` has the math (`apply_gain()`) but nothing in `buffer.rs` calls it. RG is tag-only right now.

## Where DSP Fits

### Option A: Decode thread, before ring buffer (RECOMMENDED)

```
Symphonia decode → SampleBuffer<f32>
                → DSP pipeline (biquad chain, convolution, RG)
                → rtrb::Producer
                → Ring Buffer
                → CoreAudio RT (untouched)
```

**Why this wins:**

1. **RT safety is preserved.** The CoreAudio callback stays allocation-free, lock-free, branchless. Zero risk of introducing glitches from DSP processing.
2. **Backpressure absorbs DSP cost.** If DSP takes longer per packet, the decode thread just fills the ring buffer slightly slower. The ~1s buffer (192k samples at 96kHz stereo) absorbs jitter easily.
3. **Interleaved f32 is already right there.** `sbuf.samples()` gives us `&[f32]` interleaved — perfect input for biquad processing.
4. **Gapless is unaffected.** DSP state resets per-track naturally (new file = new decoder = new DSP context), or we can persist filter state across gapless boundaries for zero transient.
5. **ReplayGain integrates trivially.** RG becomes the first stage of the DSP chain — just a gain stage. This also fixes the current gap where RG tags are read but never applied.

**Latency impact:** Negligible. DSP adds microseconds per packet to the decode thread. The ring buffer already introduces ~1s of latency — DSP adds nothing perceptible on top.

### Option B: Output thread, after ring buffer

Would require DSP in the CoreAudio render callback. **Rejected** — violates the no-allocation golden rule. Biquad IIR filters themselves are cheap enough (just multiply-accumulate), but:
- Parameter changes would need lock-free communication (atomics or second ring buffer for coefficients)
- Convolution (FIR) requires FFT which allocates scratch buffers
- Any bug = hard crash on the audio thread with no recovery

### Option C: Intermediate thread between decode and output

A third processing thread with two ring buffers (decode→DSP→output). Over-engineered for this use case. Adds complexity and latency for no benefit — the decode thread already has plenty of headroom.

## Architecture Proposal

### DSP Chain Structure

```rust
/// A single DSP processor. Process interleaved f32 samples in-place.
trait DspProcessor: Send {
    /// Process a buffer of interleaved samples.
    fn process(&mut self, samples: &mut [f32], channels: u16, sample_rate: u32);
    /// Reset internal state (on seek, track change, etc).
    fn reset(&mut self);
}

/// The full DSP pipeline — a chain of processors applied in order.
struct DspPipeline {
    processors: Vec<Box<dyn DspProcessor>>,
}

impl DspPipeline {
    fn process(&mut self, samples: &mut [f32], channels: u16, sample_rate: u32) {
        for proc in &mut self.processors {
            proc.process(samples, channels, sample_rate);
        }
    }
}
```

### Processor Types (Phase 1)

1. **GainProcessor** — simple linear gain (subsumes ReplayGain + preamp + DSP preamp)
2. **ParametricEqProcessor** — chain of biquad filters (peaking, low shelf, high shelf, low pass, high pass)
3. **ConvolutionProcessor** — FIR filter via partitioned FFT convolution (Phase 2)

### Integration Point in buffer.rs

The DSP pipeline gets applied in `decode_single()`, right after `sbuf.copy_interleaved_ref(decoded)` and before the ring buffer push loop:

```rust
// Current code:
sbuf.copy_interleaved_ref(decoded);
let samples = sbuf.samples();

// New code:
sbuf.copy_interleaved_ref(decoded);
let samples = sbuf.samples_mut();  // Need mutable access
dsp_pipeline.process(samples, channels, sample_rate);
```

**Problem:** `SampleBuffer::samples()` returns `&[f32]`, not `&mut [f32]`. Two options:
1. Copy into a `Vec<f32>` scratch buffer, process in-place, push from scratch buffer. Allocation happens once (reused across packets).
2. Use `SampleBuffer::samples_mut()` if symphonia exposes it (it doesn't currently).

Option 1 is fine — the scratch buffer is allocated once per `decode_single()` call and reused. The decode thread is not RT-constrained.

### Passing DSP Config to Decode Thread

The decode thread currently receives: `(initial_id, path, producer, seek_ms, next_track_closure, timeline)`.

Add a `DspPipeline` (or `Arc<parking_lot::RwLock<DspConfig>>` for hot-reloading):

**Cold path (simpler, Phase 1):** Pass `DspPipeline` by value into `start_decode()`. It's `Send`. Config changes require restarting the decode thread (same as seek — already instant).

**Hot path (Phase 2):** Share `Arc<AtomicPtr<DspConfig>>` or use a triple-buffer pattern. Decode thread atomically swaps to new config between packets. No lock contention.

## Recommended Rust Libraries

### Core DSP (Phase 1)

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `biquad` | 0.4+ | IIR biquad filters | `no_std`, DF1 + DF2T implementations. Based on Audio EQ Cookbook. Supports peaking, shelf, lowpass, highpass, bandpass, notch, allpass. This is the right choice — small, focused, zero deps. |

That's it for Phase 1. One crate. The biquad crate gives us:
- `Coefficients::from(Type::PeakingEQ(f0, db_gain, q))` — exactly what AutoEQ ParametricEQ.txt files specify
- `DirectForm1::run(sample)` — process one sample, maintains filter state
- All standard filter types from the Audio EQ Cookbook (Robert Bristow-Johnson)

### Convolution (Phase 2)

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `fft-convolver` | 0.2+ | Partitioned FFT convolution | Pure Rust port of HiFi-LoFi/FFTConvolver. Zero runtime allocations after init. Real-time safe `process()`. Supports runtime IR swaps. |
| `realfft` | 0.4+ | Real-valued FFT | Wrapper around `rustfft`. Only needed if we implement our own convolution or spectrum analysis. Likely not needed if `fft-convolver` covers the use case. |

### NOT recommended

| Crate | Why not |
|-------|---------|
| `dasp` | Too broad. We don't need its `Signal` trait abstractions or sample type conversions — we already have interleaved f32 from Symphonia. Adding dasp would be pulling in a framework when we need a scalpel. |
| `rubato` | Only needed if we add resampling. koan currently does bit-perfect device rate switching. If we ever need software resampling (e.g., for DSP that requires fixed sample rate), rubato is the right choice — but not Phase 1. |

## Headphone Profile Sources

### AutoEQ (Primary Source)

The [AutoEQ project](https://github.com/jaakkopasanen/AutoEq) is the goldmine. 2500+ headphone measurements, actively maintained.

**Repository structure:**
```
results/
├── oratory1990/
│   ├── over-ear/
│   │   └── Sennheiser HD 600/
│   │       ├── Sennheiser HD 600.png          (FR graph)
│   │       └── Sennheiser HD 600 ParametricEQ.txt
│   └── in-ear/
│       └── ...
├── crinacle/
│   ├── 711 in-ear/
│   └── GRAS 43AG-7 over-ear/
├── Rtings/
├── Innerfidelity/
└── ...
```

**ParametricEQ.txt format:**
```
Preamp: -6.8 dB
Filter 1: ON PK Fc 20 Hz Gain -1.3 dB Q 2.000
Filter 2: ON PK Fc 31 Hz Gain -7.0 dB Q 0.500
Filter 3: ON PK Fc 36 Hz Gain 0.7 dB Q 2.000
...
```

Fields map directly to biquad parameters:
- `PK` = Peaking EQ filter → `biquad::Type::PeakingEQ(frequency, db_gain, q_factor)`
- `LS` = Low Shelf → `biquad::Type::LowShelf(frequency, db_gain)`
- `HS` = High Shelf → `biquad::Type::HighShelf(frequency, db_gain)`
- `Preamp` = negative gain to prevent clipping → `GainProcessor`

**Raw file URL pattern:**
```
https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/{source}/{form_factor}/{model}/{model} ParametricEQ.txt
```

**INDEX.md** at `results/INDEX.md` contains a complete listing — can be fetched once and parsed as a searchable index.

### oratory1990 (Secondary Source)

High-quality measurements, but presets are published as PDFs (hard to parse). The [oratory-dl](https://github.com/sclevine/oratory-dl) project scrapes and converts them to text format. AutoEQ already includes oratory1990 measurements in its results, so we get these for free via AutoEQ.

### Crinacle (Via AutoEQ)

Crinacle's measurements are also in AutoEQ's results directory. No separate integration needed.

### Parsing Difficulty: Trivial

The ParametricEQ.txt format is dead simple to parse with basic string splitting. No need for a parsing library. A regex or manual split handles it in ~30 lines of Rust.

## Profile Management Design

### Profile Storage

```
~/.config/koan/
├── config.toml
├── config.local.toml
├── koan.db
└── profiles/
    ├── index.json              # Cached AutoEQ index (headphone name → profile path)
    ├── autoeq/                 # Downloaded AutoEQ profiles
    │   ├── sennheiser-hd600.toml
    │   └── ...
    └── custom/                 # User-created profiles
        ├── my-preset.toml
        └── ...
```

### Profile Config Format (TOML)

```toml
# ~/.config/koan/profiles/autoeq/sennheiser-hd600.toml
name = "Sennheiser HD 600 (oratory1990)"
source = "autoeq"
source_url = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/oratory1990/over-ear/Sennheiser HD 600/Sennheiser HD 600 ParametricEQ.txt"

preamp_db = -6.8

[[filters]]
type = "peaking"
frequency = 20.0
gain_db = -1.3
q = 2.0

[[filters]]
type = "peaking"
frequency = 31.0
gain_db = -7.0
q = 0.5

[[filters]]
type = "peaking"
frequency = 36.0
gain_db = 0.7
q = 2.0
```

### Config Integration

```toml
# config.toml
[playback]
replaygain = "album"

[dsp]
enabled = true
profile = "sennheiser-hd600"   # Name of profile in profiles/ dir
# OR inline:
# profile = "custom/my-preset"

# Additional user preamp on top of profile's preamp
preamp_db = 0.0
```

### Auto-Detection (Phase 3)

macOS CoreAudio exposes device names via `kAudioObjectPropertyName`. We already use this in `device.rs` (`device_name()`). The name of the default output device (e.g., "Sennheiser HD 600") can be fuzzy-matched against the AutoEQ index to suggest profiles.

Steps:
1. Get default output device name via `device_name(default_output_device())`
2. Fuzzy match against cached AutoEQ index (reuse nucleo, already a dependency)
3. Suggest top matches in a TUI picker
4. User confirms, profile is downloaded and set in config

This only works for USB DAC/headphones that report a meaningful name. 3.5mm headphones connected to a Mac show up as "MacBook Pro Speakers" — no useful info.

## Implementation Phases

### Phase 1: Parametric EQ Pipeline

**Scope:** Biquad-based parametric EQ, manual profile loading, ReplayGain integration.

**Tasks:**
1. Add `biquad` to koan-core dependencies
2. Create `audio/dsp.rs` module with:
   - `DspProcessor` trait
   - `GainProcessor` (replaces standalone RG application)
   - `ParametricEqProcessor` (chain of biquad filters)
   - `DspPipeline` (ordered chain of processors)
3. Create `audio/profile.rs` module:
   - TOML profile parsing (our format)
   - AutoEQ ParametricEQ.txt parser (import format)
   - `DspProfile` struct → builds a `DspPipeline`
4. Integrate into `buffer.rs`:
   - `decode_single()` applies DSP after decode, before ring buffer push
   - `start_decode()` accepts optional `DspPipeline`
   - Scratch buffer for in-place processing
5. Wire ReplayGain into the DSP chain:
   - `GainProcessor` as first stage (RG gain + preamp)
   - Remove the need for separate RG application code
6. Add `[dsp]` section to `Config`
7. `koan eq import <autoeq-file>` CLI command to import ParametricEQ.txt → profile TOML
8. `koan eq list` / `koan eq set <profile>` CLI commands

**New dependencies:** `biquad`

**Estimated effort:** Medium. Core DSP is ~200 lines. Profile parsing ~150 lines. Integration ~100 lines. Config/CLI ~150 lines.

### Phase 2: AutoEQ Profile Download & Search

**Scope:** Download/cache AutoEQ index, search by headphone name, auto-download profiles.

**Tasks:**
1. Fetch and cache `results/INDEX.md` from AutoEQ GitHub
2. Parse index into searchable list of `(headphone_name, measurement_source, form_factor, path)`
3. `koan eq search <query>` — fuzzy search against index (nucleo)
4. `koan eq download <headphone>` — fetch ParametricEQ.txt, convert to profile TOML, cache
5. TUI integration: new `EqPicker` mode (reuse existing picker infrastructure)
6. Hot-reload: allow changing EQ profile without restarting playback (atomic swap of DspPipeline config, applied between decode packets)

**New dependencies:** None (reqwest already available for HTTP, nucleo for fuzzy search).

**Estimated effort:** Medium. Index fetch/parse ~200 lines. Search/download ~150 lines. TUI picker ~200 lines. Hot-reload ~100 lines.

### Phase 3: Convolution & Auto-Detection

**Scope:** FIR convolution for room correction IRs, auto-detect connected audio devices.

**Tasks:**
1. Add `fft-convolver` dependency
2. `ConvolutionProcessor` implementing `DspProcessor` trait
3. Load WAV impulse response files (symphonia can decode WAV)
4. Device auto-detection: match `device_name()` against AutoEQ index
5. `koan eq auto` — suggest profile based on connected device
6. Support for loading AutoEQ's pre-computed FIR WAV files (44.1kHz and 48kHz variants)

**New dependencies:** `fft-convolver`

**Estimated effort:** Medium-High. Convolution integration ~300 lines. Device matching ~200 lines. The tricky part is handling sample rate mismatches between IR and source audio (may need rubato for resampling the IR, or just reject mismatches).

## Risks and Trade-offs

### Bit-Perfect Claim

koan currently claims bit-perfect output. DSP processing inherently modifies the signal — we need to be upfront about this. When DSP is enabled, the output is no longer bit-perfect. The config should reflect this clearly:

```toml
[dsp]
enabled = false  # Default: off. Bit-perfect by default.
```

### Floating-Point Precision

All processing is f32 (32-bit float). This gives ~24-bit dynamic range — adequate for EQ but not ideal for high-gain situations. CamillaDSP uses f64 internally for this reason. We could:
- Process the biquad chain in f64 and convert back to f32 for the ring buffer (biquad crate supports f64)
- Keep f32 and accept the precision — for headphone correction curves (gains typically -10 to +5 dB), this is fine
- **Recommendation:** Use f64 for the biquad math, f32 for I/O. The biquad crate's `DirectForm1<f64>` handles this. Convert f32→f64 before processing, f64→f32 after. Cost is trivial.

### Gapless Transitions and DSP State

When the decode thread transitions between tracks gaplessly:
- **Same format:** Keep DSP filter state alive across the boundary. Filter ringing naturally decays. This is correct behavior — same as any hardware EQ.
- **Different sample rate:** Must reset DSP state and recompute biquad coefficients for the new sample rate. Biquad coefficients are frequency-dependent — a 1kHz peaking filter at 44.1kHz has different coefficients than at 96kHz.
- **Different channel count:** Must reset DSP state. Different number of filter instances per channel.

The decode thread already tracks `StreamInfo` per file. Compare sample rate + channels across boundaries to decide reset vs. persist.

### CPU Cost

Biquad filters are dirt cheap: 5 multiply-accumulates per sample per filter. A 10-band PEQ on stereo 192kHz audio:
- 10 filters x 2 channels x 192,000 samples/sec x 5 MACs = 19.2M MACs/sec
- Modern CPU does billions of MACs/sec. This is < 1% CPU.

Convolution is heavier but still manageable with partitioned FFT. A 65,536-tap FIR at 48kHz stereo uses ~10% of one core. For headphone correction, typical FIR lengths are 4096-16384 taps — completely fine.

### Profile Quality Variance

AutoEQ profiles vary in quality by measurement source. oratory1990 measurements are generally considered gold standard. Crinacle is excellent for IEMs. Rtings is decent but uses different equipment. The UI should show the measurement source so users can make informed choices.

### Sample Rate Dependency

Biquad coefficients must be recalculated when the sample rate changes. This is fast (just math, no allocation) but must happen:
1. On track change if the new track has a different sample rate
2. On profile change

This means the `DspPipeline` needs to know the current sample rate and rebuild coefficients when it changes.

### Config Complexity

Adding `[dsp]` with profiles, preamps, and filter chains adds config surface area. Keep defaults sane:
- DSP off by default
- No profile selected by default
- `koan eq` subcommands make it approachable
- TUI picker (Phase 2) makes it discoverable

## Reference: CamillaDSP

[CamillaDSP](https://github.com/HEnquist/camilladsp) is the gold standard for Rust audio DSP. It uses realfft, rubato, and its own biquad implementation. Our architecture is simpler — we don't need mixers, crossovers, or multi-device routing. But CamillaDSP validates the approach: biquad chains + FFT convolution in Rust, running in a separate processing stage from capture/playback.

The author of CamillaDSP also wrote `realfft` and `rubato` — these are battle-tested in production audio systems.
