//! Test-only access to the actual compensated printed-audio branch.
use super::*;

/// Owns a synthetic delayed receiver for headless per-thread allocation probes.
/// Construction and destruction belong outside the measured callback scope.
pub struct PrintedBranchProbe {
    runtime: SignalRuntime,
    track: Track,
    chain: hex_lattice::NativeRuntimeChain,
    index: usize,
    frame: u64,
}

impl PrintedBranchProbe {
    /// Prepare one receiver and a seven-frame synthetic generator latency.
    pub fn prepare(
        revision: &crate::PreparedRevision,
        canonical: &hex_project::Project,
        track: TrackId,
    ) -> Result<Self, Invalid> {
        let mut project = (**revision.arrangement()).clone();
        let index = project
            .tracks
            .iter()
            .position(|row| row.canonical_track_id() == Some(track))
            .ok_or(Invalid("probe track is missing"))?;
        project.tracks[index].instrument_latency = Some(7);
        project.tracks[index].instrument_runtime_latency = Some(7);
        let graph = Arc::new(SignalGraph::prepare_value(
            &project,
            &[],
            canonical,
            &[index],
        )?);
        let track = project.tracks[index].clone();
        let chain = track
            .new_device_runtime()
            .map_err(|_| Invalid("probe chain preparation failed"))?;
        Ok(Self {
            runtime: SignalRuntime::new(graph)?,
            track,
            chain,
            index,
            frame: 0,
        })
    }

    /// Process one prepared block through the same branch as the audio renderer.
    pub fn render(&mut self, frames: &mut [Frame]) -> Result<(), Invalid> {
        self.runtime.delays.begin_transaction().map_err(error)?;
        self.chain.begin_block();
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.runtime
                .delays
                .push_transport_stamp(TransportStamp {
                    tempo_micro_bpm: None,
                    performance_frame: self.frame,
                    transport_is_running: true,
                    arrangement_frame: self.frame,
                    loop_pass: 0,
                    performance_beat_raw: 0,
                    arrangement_beat_raw: Some(0),
                })
                .map_err(error)?;
            let mixed = self.runtime.process_track(
                self.index,
                &self.track,
                &mut self.chain,
                *frame,
                [0.0; 2],
                offset,
                false,
                &mut None,
            )?;
            *frame = self.runtime.fader(&self.track, self.index, None, mixed)?;
            self.frame = self
                .frame
                .checked_add(1)
                .ok_or(Invalid("probe frame overflow"))?;
        }
        self.runtime.delays.commit_transaction().map_err(error)?;
        self.chain.commit_block();
        Ok(())
    }
}
