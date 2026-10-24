use hex_warp::Frame;
// Least-squares fundamental fit includes an independent DC term and free phase.
// A harmless constant phase change is not misclassified as choppy sustain.
pub fn residual(pcm: &[Frame], rate: u32, hz: f64) -> (f64, f64) {
    let n = pcm.len() as f64;
    let mut sum = [0.0; 3];
    let mut ss = 0.0;
    let mut cc = 0.0;
    let mut sc = 0.0;
    let mut xs = 0.0;
    let mut xc = 0.0;
    let mut xx = 0.0;
    for (i, x) in pcm.iter().enumerate() {
        let (s, c) = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin_cos();
        let x = f64::from(x[0]);
        sum[0] += x;
        sum[1] += s;
        sum[2] += c;
        ss += s * s;
        cc += c * c;
        sc += s * c;
        xs += x * s;
        xc += x * c;
        xx += x * x;
    }
    ss -= sum[1] * sum[1] / n;
    cc -= sum[2] * sum[2] / n;
    sc -= sum[1] * sum[2] / n;
    xs -= sum[0] * sum[1] / n;
    xc -= sum[0] * sum[2] / n;
    xx -= sum[0] * sum[0] / n;
    let determinant = ss * cc - sc * sc;
    let a = (xs * cc - xc * sc) / determinant;
    let b = (xc * ss - xs * sc) / determinant;
    let error = ((xx - a * xs - b * xc).max(0.0) / n).sqrt();
    (error, (xx / n).sqrt())
}
