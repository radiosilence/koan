# Target curves

Each file is a headphone frequency-response target, as `frequency,raw` on
AutoEQ's standard grid of 695 points from 20 Hz to 20 kHz, the grid AutoEQ's
results use. They are copied unchanged from AutoEQ, at commit
`7ae0f56d53074872b028649617a22bbb4232feb7`
(<https://github.com/jaakkopasanen/AutoEq/tree/7ae0f56d53074872b028649617a22bbb4232feb7/targets>).

AutoEQ is distributed under the MIT License:

> MIT License
>
> Copyright (c) 2018-2022 Jaakko Pasanen
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

| File | AutoEQ file | Origin |
|---|---|---|
| `harman-over-ear-2018.csv` | `Harman over-ear 2018.csv` | Harman International's over-ear target, from Olive, Welti and Khonsaripour's listener-preference research |
| `harman-over-ear-2018-without-bass.csv` | `Harman over-ear 2018 without bass.csv` | The same, without its low-frequency shelf, as AutoEQ derives it |
| `harman-in-ear-2019.csv` | squig.link's `Harman IE 2019 Target.txt`, not AutoEQ's (see below) | Harman International's in-ear target, from the same research group |
| `harman-in-ear-2019-without-bass.csv` | `Harman in-ear 2019 without bass.csv` | The same, without its low-frequency shelf, as AutoEQ derives it |
| `oratory1990-over-ear.csv` | `oratory1990 optimum hifi over-ear.csv` | oratory1990's over-ear target |
| `oratory1990-in-ear.csv` | `oratory1990 in-ear.csv` | oratory1990's in-ear target |
| `autoeq-in-ear.csv` | `AutoEq in-ear.csv` | AutoEQ's own in-ear target |
| `diffuse-field-gras-kemar.csv` | `Diffuse field GRAS KEMAR.csv` | A diffuse-field target for GRAS/KEMAR ear simulators, as AutoEQ publishes it |
| `diffuse-field-iso-11904-1.csv` | `Diffuse field ISO 11904-1.csv` | The diffuse-field response at the eardrum from ISO 11904-1, as AutoEQ publishes it: the neutral reference for in-ears measured on an IEC 60318-4 (711) coupler, which approximates the eardrum |

`harman-in-ear-2019.csv` is squig.link's own Harman in-ear 2019 target
(<https://squig.link/data/Harman%20IE%202019%20Target.txt>, fetched
2026-10-07), interpolated onto AutoEQ's grid against log frequency and held
flat past its last point at 19.75 kHz. A correction to it then aims where the
auto-EQ presets squig.link makes do. It lies within 0.03 dB RMS of AutoEQ's
file; the two part only above 19.75 kHz, by up to 0.6 dB. The unchanged file
is kept as `../testdata/squig-harman-ie-2019-target.txt`, which a test holds
this one to.

`flat.csv` is koan's own: 0 dB at every point of the grid. It is the
speaker target, the convention of CTA-2034 and spinorama measurements, which
aim for a flat listening window; a room tilt is a tuning on top.

Targets a person adds themselves (a CSV, or a squig.link export) are kept
beside their config under `dsp/targets/`, not here, and are theirs to license.
