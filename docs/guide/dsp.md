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
device, that device's settings apply.

A UPnP renderer is an output like any other and can have a profile of its own.
Without one it is handed the original file, untouched. With one, kōan decodes
and processes the queue itself and sends the renderer a single FLAC stream for
as long as the format stays the same, dithered to the source's bit depth (24
bits for lossy sources). The renderer never changes track inside that stream,
so gapless holds, and with a single convolution filter every track plays at the
filter's rate and the stream never ends between tracks. Seeking opens a new
stream at the new position. A renderer that cannot take FLAC is sent WAV;
one that takes neither is handed the original file.

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

### From AutoEQ

AutoEQ's corrections can be found by headphone name instead of downloaded by
hand. In the Mac and iOS apps, **Find in AutoEQ…** under EQ and convolution
searches as you type, or, before anything is typed, lists the makers and
under each its models, for a headphone whose name does not come to mind;
choosing a result installs it and plays the output in use through it.

When the output's own name ends with a headphone's whole name as AutoEQ
gives it, maker included ("Jo's Sony WH-1000XM4"), the same section offers
AutoEQ's profile for it. The rule is strict because a wrong correction is
worse than none: a name with a generation the index lacks ("Apple AirPods
Pro 3") is offered nothing, and so are audio interfaces and DACs whose model
happens to share a headphone's ("MOTU M2", "Hugo 2"). A short list of models
that name their own generation may leave the maker out, as their Bluetooth
names do: Sony's WH-1000X and WF-1000X lines and LinkBuds, AirPods Max, and
Samsung's Galaxy Buds2 and Buds3. Plain "AirPods" and "AirPods Pro" are not on
it, since every generation calls itself that, nor "AirPods 4", which is sold
with and without noise cancelling under the one name. For those, and any name that
says roughly which headphone it is without naming the entry, the offer is
**Find <model> in AutoEQ…** instead: the search opens on the model, with its
generations and variants listed to pick from. Find in AutoEQ… covers the rest. Nothing is
applied until you choose to, and turning the offer down for a device is
remembered in `config.local.toml` (`dsp.autoeq_dismissed`). An output with a
profile of its own is not offered one.

From the command line, `koan dsp autoeq search` matches names fuzzily against AutoEQ's index and
lists each result with who measured it; `install` takes a result's number, or
its exact name, and saves its parametric EQ as a profile named
`<model> (AutoEQ, <source>)`:

```bash
koan dsp autoeq search hd650
koan dsp autoeq install 6258 --device "Topping E30"
koan dsp autoeq install "Sennheiser HD 650" --source crinacle
```

Where several sources measured the same headphone, a name alone installs the
one AutoEQ lists first, which is the one it recommends. The index (about
850 KB) is kept in the config directory under `autoeq/` and fetched again at
most once a day, by ETag, so an unchanged index costs one empty response;
`search --refresh` asks regardless. When GitHub cannot be reached, the copy
kept is used. Numbers refer to that copy, so `install` never refreshes it.

A profile is named after what it came from; rename it on its page in Settings
(or pass `--name`). Importing into a profile of the same name adds to it, so a
room's responses and a headphone EQ can live in one profile.

| Format | From | How rates and channels are matched |
|---|---|---|
| WAV, AIFF, FLAC or ALAC impulse responses | REW, rePhase, Acourate, AutoEQ's FIR export, Roon packs | Each file's own rate. A mono file applies to every channel; otherwise one channel per output channel. Mono files at one rate are matched to channels by `L`/`R` in their names |
| Roon zip | Roon, Home Audio Fidelity | Unpacked and read as the files inside: responses as above, or `.cfg` files |
| Convolver `.cfg` | Roon, JRiver, Acourate, Audiolense | The header's rate; routes say which response feeds which channel, at what weight and delay. Crossfeed works; a crossover to more outputs than inputs is refused |
| CamillaDSP YAML | CamillaDSP | `devices.samplerate`. Biquads, gains, delays and mixers keep their place in the pipeline, on the channels it gives them; `Conv` filters become responses |
| Equalizer APO `config.txt`, AutoEQ `ParametricEQ.txt` and `GraphicEQ.txt`, squig.link exports, REW filter settings | Equalizer APO, AutoEQ, squig.link, REW | Bands, `Delay:`, `Copy:` and `GraphicEQ:` in order, on the channels `Channel:` selects; `Convolution:` responses at their own rate; `Include:` followed. REW's per-speaker files go to the channel their name says |
| Raw or text coefficients | CamillaDSP, BruteFIR, REW text export | A rate in the file name (`room-48k.txt`), or asked for |

Filters run in the order the configuration lists them, because a mixer makes
channels out of others: a band before it is not the same as one after it.
Impulse responses run after everything else. Bands and delays act on one
channel at a time, so they give the same result on either side of a response;
a mixer does not, and one placed after a response is refused.

A `GraphicEQ:` curve, as AutoEQ's `GraphicEQ.txt` and squig.link's "Export
Graphic EQ" write one, becomes a minimum-phase FIR designed for the rate
playback runs at, with the gain between points interpolated against log
frequency. Minimum phase is what the parametric filters such a curve is
sampled from have: no pre-ringing, and no delay. Where the parametric version
is to hand it is still the better import, being the filters themselves rather
than a sampling of them.

Anything in these that kōan cannot do stops the import with its name, rather
than being dropped and leaving the correction different from what was designed.
That covers a mixer with more outputs than inputs (kōan plays as many channels
as the source has, so a crossover to four outputs has nowhere to go), a mixer
after a convolution, settings that depend on the sample rate (Equalizer APO's
`If:`), and filter types with no equivalent here, such as raw IIR
coefficients.

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

## Precision

Everything between the decoder and the device runs in 64-bit floating point:
resampling, the bands and convolution, with one conversion in and one out. A
test subtracts a textbook convolution, summed tap by tap, from kōan's output;
what is left is −152 dB, the rounding of the 32-bit samples the output takes.
A 262,145-tap Roon filter at 192 kHz, stereo, runs at 37 times real time on one
core of an M-series Mac.

## Headroom

EQ boosts and convolution can push a sample past full scale. Unless a profile
sets `preamp_db` itself, kōan works out the largest gain its filters apply at
any frequency and lowers the level by that much first, as AutoEQ's own `Preamp`
line does. ReplayGain is applied before it, with its own peak limiting, so the
two do not compound. A profile's page shows the figure.

## Choosing

Settings lists the profiles, with a tick on the one the output in use plays
through, and a picker to change it. On the Mac the Play on menu, under the
speaker in the transport bar, gives each output its preset: the row says which
("Off", or "Original file" for a renderer), and the slider button beside it
changes it, for that output whether or not it is the one playing. While what is
heard is processed, the speaker carries a dot and the format badge names the
processing. Each profile's page shows exactly what it
holds — every response's rate, channels, length, where it peaks and whether it
mixes or delays channels, any bands, the headroom, and where it was imported
from. Changes apply straight away, where playback is.

On iOS, profiles follow the route: AirPods, wired headphones and the speaker are
each their own output. Now Playing shows the preset of the output playing, beside
the AirPlay button: the route's, or a UPnP renderer's while the phone plays to
one. Tapping it picks another for that output, which applies at once. It changes
when the output does. While processing is off everywhere,
the preset menu says so on both platforms and offers to turn it back on.

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
    { type = "delay", ms = 0.25, channels = [0] },
    { type = "mix", outputs = [[[0, 0.9], [1, 0.1]], [[0, 0.1], [1, 0.9]]] },
    { type = "graphic", points = [[20.0, -1.5], [1000.0, 0.0], [10000.0, 2.0]] },
]
```

`impulses` entries are WAVs (any audio file) or Convolver `.cfg` files, relative
to the config directory. `filters` run in order, before the responses.
`channels` counts from 0 and, left out, means every channel.

| Type | Fields |
|---|---|
| `peaking`, `low_shelf`, `high_shelf`, `low_pass`, `high_pass`, `notch`, `band_pass`, `all_pass` | `freq`, `gain_db`, `q` |
| `low_shelf_first_order`, `high_shelf_first_order` | `freq`, `gain_db`: 6 dB per octave, half the gain at `freq` |
| `low_pass_first_order`, `high_pass_first_order`, `all_pass_first_order` | `freq` |
| `gain` | `gain_db` |
| `delay` | `ms` and `samples`, added together; `subsample = true` keeps the fraction of a sample rather than rounding it |
| `mix` | `outputs`: for each channel from 0, the `[input, linear gain]` pairs it is made of, every input read before any is written. An empty list is silence; channels past the list pass unchanged |
| `graphic` | `points`: `[Hz, dB]` pairs |

A filter at or above half the sample rate cannot be built and is skipped at
that rate, with a line in the log.
