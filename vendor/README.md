# Patched dependencies

Crates copied from crates.io and patched here, through `[patch.crates-io]` in the workspace manifest, until upstream releases the fix. Each is otherwise the published release, unchanged.

## opus-decoder 0.1.1

`celt::kiss_fft::flat_fft_forward`, the FFT inside CELT's inverse MDCT, was a direct O(N²) DFT that called `f64::sin_cos` for every term. Decoding a stereo Opus stream at 48 kHz cost roughly a third of a phone CPU core, which drained an iPhone overnight while playing in the background. It now runs a `rustfft` plan cached per size per thread; `planned_fft_matches_the_direct_dft_at_celt_sizes` checks it against the direct DFT at CELT's sizes.

Remove this copy and the `[patch.crates-io]` entry once a release of [Rusopus](https://github.com/TadeuszWolfGang/Rusopus) includes an equivalent fix.
