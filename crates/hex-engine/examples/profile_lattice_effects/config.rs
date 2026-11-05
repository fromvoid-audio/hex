//! Example-only bounds: these limit profiling work/storage, never runtime admission.
use super::Fallible;

#[derive(Clone, Copy, Debug)]
pub(super) struct Config {
    pub rate: u32,
    pub block: usize,
    pub tracks: usize,
    pub seconds: u32,
    pub warmup: usize,
    pub devices: Option<usize>,
    pub meters: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            rate: 48_000,
            block: 128,
            tracks: 100,
            seconds: 60,
            warmup: 100,
            devices: None,
            meters: false,
        }
    }
}
impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Fallible<Self> {
        let mut config = Self::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            if flag == "--meters" {
                config.meters = true;
                continue;
            }
            let value = args.next().ok_or("missing profile option value")?;
            match flag.as_str() {
                "--rate" => config.rate = value.parse()?,
                "--block" => config.block = value.parse()?,
                "--tracks" => config.tracks = value.parse()?,
                "--seconds" => config.seconds = value.parse()?,
                "--warmup" => config.warmup = value.parse()?,
                "--devices" => config.devices = Some(value.parse()?),
                _ => return Err(format!("unknown profile option: {flag}").into()),
            }
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(self) -> Fallible<()> {
        if ![44_100, 48_000, 96_000].contains(&self.rate)
            || ![128, 512].contains(&self.block)
            || !(1..=100).contains(&self.tracks)
            || !(1..=300).contains(&self.seconds)
            || self.warmup > 1_000
            || self
                .devices
                .is_some_and(|n| ![0, 1, 2, 3, 4, 16].contains(&n))
        {
            return Err("profile bounds: rate 44100/48000/96000, block 128/512, tracks 1..100, seconds 1..300, warmup 0..1000, devices 0/1/2/3/4/16".into());
        }
        Ok(())
    }
    pub fn measured_frames(self) -> usize {
        self.rate as usize * self.seconds as usize
    }
    pub fn warmup_frames(self) -> usize {
        self.warmup * self.block
    }
    pub fn blocks(self) -> usize {
        self.measured_frames().div_ceil(self.block)
    }
}
