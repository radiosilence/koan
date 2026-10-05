#![allow(dead_code)]

//! Minimal CELT FFT primitives for decoder path.
//!
//! This module intentionally implements only the inverse transform behavior
//! needed by the decoder. It follows the libopus scaling convention where the
//! inverse FFT is unscaled and the forward path applies `1/N`.

use core::f32::consts::PI;

/// Complex number used by CELT FFT/MDCT primitives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Complex32 {
    /// Real component.
    pub re: f32,
    /// Imaginary component.
    pub im: f32,
}

impl Complex32 {
    /// Create a complex value from real and imaginary parts.
    ///
    /// Params: `re` is the real component, `im` is the imaginary component.
    /// Returns: a new `Complex32`.
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }
}

/// Inverse-only FFT plan for CELT decoder usage.
#[derive(Debug, Clone)]
pub(crate) struct KissFft {
    nfft: usize,
    inv_twiddles: Vec<Complex32>,
}

impl KissFft {
    /// Build an inverse FFT plan with runtime twiddle generation.
    ///
    /// Params: `nfft` is the FFT size.
    /// Returns: a plan containing precomputed inverse twiddles.
    pub fn new(nfft: usize) -> Self {
        let mut inv_twiddles = Vec::with_capacity(nfft);
        for k in 0..nfft {
            let phase = 2.0 * PI * (k as f32) / (nfft as f32);
            inv_twiddles.push(Complex32::new(phase.cos(), phase.sin()));
        }
        Self { nfft, inv_twiddles }
    }

    /// Return FFT size configured in this plan.
    ///
    /// Params: none.
    /// Returns: number of points in the transform.
    pub fn len(&self) -> usize {
        self.nfft
    }

    /// Compute unscaled inverse FFT.
    ///
    /// Params: `input` is frequency-domain complex spectrum, `output` is
    /// destination time-domain buffer and must have length `self.len()`.
    /// Returns: `Ok(())` on success, otherwise a static validation error.
    pub fn ifft(&self, input: &[Complex32], output: &mut [Complex32]) -> Result<(), &'static str> {
        if input.len() != self.nfft || output.len() != self.nfft {
            return Err("kiss_fft length mismatch");
        }
        for out in output.iter_mut() {
            *out = Complex32::new(0.0, 0.0);
        }
        for (n, out) in output.iter_mut().enumerate() {
            let mut acc_re = 0.0f32;
            let mut acc_im = 0.0f32;
            for (k, xk) in input.iter().enumerate() {
                let tw = self.inv_twiddles[(k * n) % self.nfft];
                acc_re += xk.re * tw.re - xk.im * tw.im;
                acc_im += xk.re * tw.im + xk.im * tw.re;
            }
            *out = Complex32::new(acc_re, acc_im);
        }
        Ok(())
    }

    /// Compute unscaled forward FFT.
    ///
    /// Params: `input` is time-domain complex vector, `output` is destination
    /// frequency-domain buffer and must have length `self.len()`.
    /// Returns: `Ok(())` on success, otherwise a static validation error.
    pub fn fft(&self, input: &[Complex32], output: &mut [Complex32]) -> Result<(), &'static str> {
        if input.len() != self.nfft || output.len() != self.nfft {
            return Err("kiss_fft length mismatch");
        }
        for out in output.iter_mut() {
            *out = Complex32::new(0.0, 0.0);
        }
        for (n, out) in output.iter_mut().enumerate() {
            let mut acc_re = 0.0f32;
            let mut acc_im = 0.0f32;
            for (k, xk) in input.iter().enumerate() {
                let tw = self.inv_twiddles[(k * n) % self.nfft];
                // Conjugate inverse twiddle gives forward phase.
                acc_re += xk.re * tw.re + xk.im * tw.im;
                acc_im += -xk.re * tw.im + xk.im * tw.re;
            }
            *out = Complex32::new(acc_re, acc_im);
        }
        Ok(())
    }
}

/// Compute forward DFT on flat interleaved complex buffers.
///
/// Params: `input`/`output` are `[re0, im0, re1, im1, ...]` and `n` is the
/// number of complex samples.
/// Returns: nothing; writes the unscaled forward transform to `output`.
///
/// Runs a planned FFT, cached per size on each thread. CELT's sizes are few
/// (`n/4` of the MDCT lengths), so the cache stays a handful of plans.
pub(crate) fn flat_fft_forward(input: &[f32], output: &mut [f32], n: usize) {
    use rustfft::num_complex::Complex;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct Plans {
        planner: rustfft::FftPlanner<f32>,
        plans: HashMap<usize, Arc<dyn rustfft::Fft<f32>>>,
        buffer: Vec<Complex<f32>>,
        scratch: Vec<Complex<f32>>,
    }

    thread_local! {
        static PLANS: RefCell<Option<Plans>> = const { RefCell::new(None) };
    }

    assert!(input.len() >= 2 * n && output.len() >= 2 * n);
    if n == 0 {
        return;
    }
    PLANS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let p = cell.get_or_insert_with(|| Plans {
            planner: rustfft::FftPlanner::new(),
            plans: HashMap::new(),
            buffer: Vec::new(),
            scratch: Vec::new(),
        });
        let fft = match p.plans.get(&n) {
            Some(fft) => fft.clone(),
            None => {
                let fft = p.planner.plan_fft_forward(n);
                p.plans.insert(n, fft.clone());
                fft
            }
        };
        p.buffer.clear();
        p.buffer
            .extend(input[..2 * n].chunks_exact(2).map(|c| Complex::new(c[0], c[1])));
        p.scratch
            .resize(fft.get_inplace_scratch_len(), Complex::new(0.0, 0.0));
        fft.process_with_scratch(&mut p.buffer, &mut p.scratch);
        for (out, c) in output[..2 * n].chunks_exact_mut(2).zip(&p.buffer) {
            out[0] = c.re;
            out[1] = c.im;
        }
    });
}

#[cfg(test)]
mod tests {

    /// The direct DFT `flat_fft_forward` used to be, as the reference.
    fn direct_dft(input: &[f32], n: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; 2 * n];
        for k in 0..n {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for j in 0..n {
                let angle = -2.0 * core::f64::consts::PI * (k as f64) * (j as f64) / (n as f64);
                let (s, c) = angle.sin_cos();
                let (x, y) = (input[2 * j] as f64, input[2 * j + 1] as f64);
                re += x * c - y * s;
                im += x * s + y * c;
            }
            out[2 * k] = re as f32;
            out[2 * k + 1] = im as f32;
        }
        out
    }

    #[test]
    fn planned_fft_matches_the_direct_dft_at_celt_sizes() {
        for n in [60usize, 120, 240, 480] {
            let input: Vec<f32> = (0..2 * n)
                .map(|i| ((i as f32 * 0.7319).sin() * 0.9 + (i as f32 * 0.113).cos() * 0.3))
                .collect();
            let mut fast = vec![0.0f32; 2 * n];
            super::flat_fft_forward(&input, &mut fast, n);
            let want = direct_dft(&input, n);
            let scale = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            for (a, b) in fast.iter().zip(&want) {
                assert!((a - b).abs() <= scale * 1e-5, "n={n}: {a} vs {b}");
            }
        }
    }

    use super::{Complex32, KissFft};
    use core::f32::consts::PI;

    /// Compute libopus-style forward DFT with `1/N` scaling for tests.
    ///
    /// Params: `input` is time-domain complex vector.
    /// Returns: scaled frequency-domain vector.
    fn forward_scaled(input: &[Complex32]) -> Vec<Complex32> {
        let n = input.len();
        let mut out = vec![Complex32::new(0.0, 0.0); n];
        for (k, yk) in out.iter_mut().enumerate() {
            let mut acc_re = 0.0f32;
            let mut acc_im = 0.0f32;
            for (n_idx, xn) in input.iter().enumerate() {
                let phase = -2.0 * PI * (k as f32) * (n_idx as f32) / (n as f32);
                let c = phase.cos();
                let s = phase.sin();
                acc_re += xn.re * c - xn.im * s;
                acc_im += xn.re * s + xn.im * c;
            }
            *yk = Complex32::new(acc_re / (n as f32), acc_im / (n as f32));
        }
        out
    }

    #[test]
    fn ifft_roundtrip_matches_input() {
        let n = 60usize;
        let fft = KissFft::new(n);
        let mut input = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / n as f32;
            input.push(Complex32::new(
                (2.0 * PI * 3.0 * t).sin(),
                (2.0 * PI * 5.0 * t).cos(),
            ));
        }
        let freq = forward_scaled(&input);
        let mut recon = vec![Complex32::new(0.0, 0.0); n];
        fft.ifft(&freq, &mut recon).expect("ifft must succeed");
        for (a, b) in input.iter().zip(recon.iter()) {
            assert!(
                (a.re - b.re).abs() < 2e-5,
                "re mismatch: {} vs {}",
                a.re,
                b.re
            );
            assert!(
                (a.im - b.im).abs() < 2e-5,
                "im mismatch: {} vs {}",
                a.im,
                b.im
            );
        }
    }
}
