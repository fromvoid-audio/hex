//! Spectral aliasing oracle for oversampled devices (tests only).

/// In-place iterative radix-2 FFT (test oracle only).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}
/// Total non-harmonic spectral energy relative to the fundamental, in db.
/// A 4-term Blackman-Harris window (-92 db sidelobes) over the last 65536
/// frames; bins within 8 of any harmonic of `hz` (or dc) count as harmonic.
pub(crate) fn inharmonic_energy_db(output: &[hex_lattice::StereoFrame], hz: f64, rate: u32) -> f64 {
    const N: usize = 1 << 16;
    let frames = &output[output.len() - N..];
    let mut re: Vec<f64> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let x = std::f64::consts::TAU * i as f64 / N as f64;
            let w =
                0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos();
            f64::from(f[0]) * w
        })
        .collect();
    let mut im = vec![0.0; N];
    fft(&mut re, &mut im);
    let bin_hz = f64::from(rate) / N as f64;
    let power = |k: usize| re[k] * re[k] + im[k] * im[k];
    let fundamental_bin = (hz / bin_hz).round() as usize;
    let fundamental: f64 = (fundamental_bin - 8..=fundamental_bin + 8).map(power).sum();
    let mut inharmonic = 0.0_f64;
    for k in 9..N / 2 {
        let f = k as f64 * bin_hz;
        let nearest = (f / hz).round() * hz;
        if ((f - nearest) / bin_hz).abs() <= 8.0 {
            continue;
        }
        inharmonic += power(k);
    }
    10.0 * (inharmonic / fundamental).log10()
}
