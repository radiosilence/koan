# Equalisation and convolution

kōan can correct headphones, speakers and rooms on its own output, with filters
designed elsewhere: headphone and IEM EQ from AutoEQ or squig.link, for players
and DACs with no EQ of their own; room-correction impulse responses from REW,
rePhase, Acourate, Audiolense or Home Audio Fidelity, Roon filter packs,
CamillaDSP and Equalizer APO setups. A profile belongs to output devices, so
plugging in the headphones selects their correction, and a device no profile
names plays bit-perfect as before.

The processing runs on the decode thread, before the ring buffer, so the audio
callback is the same with DSP on as off. With DSP off, or on a device without a
profile, nothing runs at all: the samples reach the device untouched, and the
format badge says when they did not ("FLAC 24/96 · FIR").

Processing applies to this device's own output. When it controls another
device, or plays to a renderer, that device's settings apply.

## Importing

In the apps, **Settings → Playback → EQ and convolution → Import…** takes files,
a folder or a zip. On iOS, files can also come from anywhere else: open one in
kōan from Files or Mail, or choose kōan in the share sheet — a zip sent in a
chat, or EQ text pasted into a message. A share is saved for the app and
imported when kōan next comes to the front.

From the command line:

```bash
koan dsp import "Harman 780.zip" --device "Topping E30"
```

A profile is named after what it came from; rename it on its page in Settings
(or pass `--name`). Importing into a profile of the same name adds to it, so a
room's responses and a headphone EQ can live in one profile.

| Format | From | How rates and channels are matched |
|---|---|---|
| WAV, AIFF, FLAC or ALAC impulse responses | REW, rePhase, Acourate, AutoEQ's FIR export, Roon packs | Each file's own rate. A mono file applies to every channel; otherwise one channel per output channel. Mono files at one rate are matched to channels by `L`/`R` in their names |
| Roon zip | Roon, Home Audio Fidelity | Unpacked and read as the files inside: responses as above, or `.cfg` files |
| Convolver `.cfg` | Roon, JRiver, Acourate, Audiolense | The header's rate; routes say which response feeds which channel, at what weight and delay. Crossfeed works; a crossover to more outputs than inputs is refused |
| CamillaDSP YAML | CamillaDSP | `devices.samplerate`. Biquads become bands and `Conv` filters responses, on the channels the pipeline gives them. Mixers are refused |
| Equalizer APO `config.txt`, AutoEQ `ParametricEQ.txt`, squig.link `Filters.txt`, REW filter settings | Equalizer APO, AutoEQ, squig.link, REW | Bands on the channels `Channel:` selects; `Convolution:` responses at their own rate; `Include:` followed. REW's per-speaker files go to the channel their name says |
| Raw or text coefficients | CamillaDSP, BruteFIR, REW text export | A rate in the file name (`room-48k.txt`), or asked for |

Anything in these that kōan cannot do — a mixer, a delay filter, a first-order
shelf, Equalizer APO's `Copy:` — stops the import with its name, rather than
being dropped and leaving the correction different from what was designed.
A `GraphicEQ:` file — AutoEQ's `GraphicEQ.txt`, squig.link's "Export Graphic
EQ" — is a curve sampled from filters; import the parametric version, which
holds the filters themselves.

kōan keeps what it imported under `dsp/<profile>/` beside the config, as one
32-bit float WAV per rate, with a `.cfg` where the routes mix or delay channels.
The originals are not needed again.

## Sample rates

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
two do not compound. A profile's page shows the figure.

## Choosing

Settings lists the profiles, with a tick on the one the output in use plays
through, and a picker to change it. Each profile's page shows exactly what it
holds — every response's rate, channels, length, where it peaks and whether it
mixes or delays channels, any bands, the headroom, and where it was imported
from. Changes apply straight away, where playback is.

On iOS, profiles follow the route: AirPods, wired headphones and the speaker are
each their own output.

```bash
koan dsp                        # list; * marks the current output's profile
koan dsp use "Living room"      # the current output (or --device NAME) plays through it
koan dsp clear                  # the current output plays untouched
koan dsp off | on               # bypass every profile, or stop bypassing
koan dsp remove "Living room"
```

## Configuration

Profiles describe the listening setup rather than taste, so they live in
`config.local.toml`. Importing writes them there; they are plain TOML to edit by
hand too:

```toml
[dsp]
enabled = true    # false bypasses every profile without deleting any

[[dsp.profiles]]
name = "Living room"
devices = ["Topping E30"]
impulses = ["dsp/living-room/44100.wav", "dsp/living-room/48000.cfg"]
# preamp_db = -6.0  # leave unset to derive it from the filters
filters = [
    { type = "peaking", freq = 46.5, gain_db = -9.4, q = 4.47 },
    { type = "low_shelf", freq = 105.0, gain_db = 5.5, q = 0.7, channels = [1] },
]
```

`impulses` entries are WAVs (any audio file) or Convolver `.cfg` files, relative
to the config directory. Filter types are `peaking`, `low_shelf`, `high_shelf`,
`low_pass`, `high_pass`, `notch`, `band_pass`, `all_pass` and `gain`; `channels`
counts from 0 and, left out, means every channel. A band at or above half the
sample rate cannot be built and is skipped at that rate, with a line in the log.
