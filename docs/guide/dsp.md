# Equalisation and convolution

kōan can correct headphones and speakers on its own output: parametric EQ (an
AutoEQ profile, or bands of your own) and FIR convolution (room correction
filters designed in REW, rePhase, Dirac or for CamillaDSP). A profile belongs to
output devices, so plugging in the headphones selects their correction, and a
device no profile names plays bit-perfect as before.

The processing runs on the decode thread, before the ring buffer, so the audio
callback is the same with DSP on as off. With DSP off, or on a device without a
profile, nothing runs at all: the samples reach the device untouched, and the
format badge says when they did not ("FLAC 24/96 · EQ").

Processing applies to this device's own output. When it controls another
device, or plays to a renderer, that device's settings apply.

## Headphone EQ from AutoEQ

Find your headphones in [AutoEQ](https://github.com/jaakkopasanen/AutoEq/tree/master/results)
and download the `ParametricEQ.txt`. Then:

```bash
koan dsp import "Sennheiser HD 600 ParametricEQ.txt" --device "Topping E30"
```

The profile is named after the file (`--name` to choose), and `--device` plays
that output through it; leave it out and use `koan dsp use <name>` later, which
picks the current output device unless given `--device`. `koan devices` lists
the names.

## Room correction

Export the filter from your design tool as a WAV impulse response, once for
each sample rate you play. Most tools can: REW and rePhase export per rate, and
Dirac and CamillaDSP setups usually already have a set.

```bash
koan dsp impulse "Living room" room-44k.wav room-48k.wav room-88k.wav room-96k.wav
koan dsp use "Living room" --device "Arcam SA30"
```

Each file's own sample rate is the rate it applies to. A WAV with one channel is
applied to every channel; otherwise it needs one channel per output channel.

An impulse response is only correct at the rate it was designed for. When a
track's rate has a response of its own, it is used and nothing is resampled.
When it has none, kōan resamples the track to the nearest rate it has a
response for and plays at that rate, and the badge says so ("FLAC 24/96 → 48 ·
FIR"). Resampling the filter instead would be worse: a filter designed at 48 kHz
holds nothing above 24 kHz, so on hi-res material it acts as a low-pass. With a
single response everything plays at one rate, so the device never changes rate
between tracks and gapless holds across tracks of different rates.

A linear-phase filter delays the audio by its group delay, often tens of
milliseconds. kōan trims that delay from the start of playback and plays it out
at the end, so the seek bar and synced lyrics line up with what is heard.

## Headroom

EQ boosts and convolution can push a sample past full scale. Unless a profile
sets `preamp_db` itself, kōan works out the largest gain its filters apply at
any frequency and lowers the level by that much first, as AutoEQ's own `Preamp`
line does. ReplayGain is applied before it, with its own peak limiting, so the
two do not compound.

## Configuration

Profiles describe the listening setup rather than taste, so they live in
`config.local.toml`. `koan dsp` writes them there; they are plain TOML to edit by
hand too:

```toml
[dsp]
enabled = true    # false bypasses every profile without deleting any

[[dsp.profiles]]
name = "HD 600"
devices = ["Topping E30"]
# preamp_db = -6.0  # leave unset to derive it from the filters
filters = [
    { type = "peaking", freq = 20.0, gain_db = -1.3, q = 2.0 },
    { type = "low_shelf", freq = 105.0, gain_db = 5.5, q = 0.7 },
]
impulses = ["room-48k.wav"]  # relative paths are read from beside the config
```

Filter types are `peaking`, `low_shelf`, `high_shelf`, `low_pass` and
`high_pass`. A band at or above half the sample rate cannot be built and is
skipped at that rate, with a line in the log.

Changes apply the next time a track starts or is seeked; a track already playing
carries on with the profile it started with.

| Command | Does |
|---|---|
| `koan dsp` | List profiles; `*` marks the one the current output plays through |
| `koan dsp import <file>` | Make a profile from a `ParametricEQ.txt` |
| `koan dsp impulse <name> <wav>…` | Set a profile's impulse responses |
| `koan dsp use <name>` | Play an output device through a profile |
| `koan dsp clear` | Play an output device untouched |
| `koan dsp remove <name>` | Delete a profile |
| `koan dsp off` / `on` | Bypass every profile, or stop bypassing |
