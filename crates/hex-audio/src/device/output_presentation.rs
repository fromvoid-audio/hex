//! Bind actual Core Audio timing to the worker-owned output and its fault epoch.
//! All queries run on control and borrow the original stream only.
use super::WorkerPreparedOutput;
use crate::prepared::PluginOutputFence;
use cpal::platform::{CoreAudioOutputTiming, CoreAudioOutputTimingReceipt, StreamInner};

/// Opaque evidence from a plugin-capable live output. Resolve it on the same
/// output before using it as an SDK contribution. Replacement, fault recovery,
/// route notification or stream retirement can invalidate it.
#[derive(Clone, Copy, Debug)]
pub struct PluginOutputObservation {
    native: CoreAudioOutputTimingReceipt,
    output: PluginOutputFence,
}

impl WorkerPreparedOutput {
    /// Start (or replace) this stream's declared output timing observation.
    /// Unknown/unsupported output is represented by absence. This is opt-in;
    /// do not call it for every source or callback. Refresh replaces all
    /// previous receipts for the same output.
    pub fn observe_plugin_output_on_control(&self) -> Option<PluginOutputObservation> {
        let output = self
            .publication
            .owner_ref()?
            .plugin_output_fence_on_control()?;
        let stream = match self._stream.stream_on_control()?.as_inner() {
            StreamInner::CoreAudio(stream) => stream,
            #[allow(unreachable_patterns)] // Other backends are optional on macOS.
            _ => return None,
        };
        let observation = PluginOutputObservation {
            native: stream.watch_output_timing_on_control().ok()?,
            output,
        };
        self.resolve_plugin_output_on_control(observation)?;
        Some(observation)
    }

    /// Revalidate the actual original route and the publication fence before
    /// resolving its declared output contribution in project frames. This is
    /// a point-in-time observation. Persistent live plans must react to its
    /// invalidation. Graph, bridge and plugin processing delay are separate.
    pub fn resolve_plugin_output_on_control(
        &self,
        observation: PluginOutputObservation,
    ) -> Option<u32> {
        let control = self.publication.owner_ref()?;
        if control.plugin_output_fence_on_control()? != observation.output {
            return None;
        }
        let stream = match self._stream.stream_on_control()?.as_inner() {
            StreamInner::CoreAudio(stream) => stream,
            #[allow(unreachable_patterns)] // Other backends are optional on macOS.
            _ => return None,
        };
        let timing = stream
            .revalidate_output_timing_on_control(observation.native)
            .ok()?;
        let frames = declared_output_frames(
            timing,
            observation.output.project_rate,
            observation.output.quantum,
        )?;
        (control.plugin_output_fence_on_control()? == observation.output).then_some(frames)
    }
}

fn declared_output_frames(
    timing: CoreAudioOutputTiming,
    project_rate: u32,
    quantum: u16,
) -> Option<u32> {
    // Admit same-rate fixed-buffer paths whose hardware buffer is an exact
    // positive multiple of the plugin bridge quantum (1:1 or sub-sliced, e.g.
    // 512 = 2 * 256 or 1024 = 4 * 256). A nonzero AU value could overlap the
    // hardware path, so leave its composition unknown.
    let quantum_u32 = u32::from(quantum);
    if quantum_u32 == 0
        || timing.client_sample_rate != f64::from(project_rate)
        || timing.device_sample_rate != f64::from(project_rate)
        || timing.buffer_frames < quantum_u32
        || !timing.buffer_frames.is_multiple_of(quantum_u32)
        || timing.audio_unit_seconds != 0.0
    {
        return None;
    }
    timing.hardware_path_client_frames().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_supported_hardware_path_contributes() {
        let observed = CoreAudioOutputTiming {
            device_id: 7,
            device_sample_rate: 48_000.0,
            client_sample_rate: 48_000.0,
            buffer_frames: 128,
            safety_frames: 13,
            device_frames: 12,
            stream_frames: 9,
            audio_unit_seconds: 0.0,
        };
        assert_eq!(declared_output_frames(observed, 48_000, 128), Some(162));
        assert_eq!(declared_output_frames(observed, 44_100, 128), None);
        assert_eq!(declared_output_frames(observed, 48_000, 256), None);
        assert_eq!(declared_output_frames(observed, 48_000, 0), None);

        // Exact sub-sliced CoreAudio hardware buffers (e.g. 44.1 kHz / 512 or 1024
        // frames sub-sliced into 256-frame plugin quanta) count the hardware buffer once.
        let subsliced_512 = CoreAudioOutputTiming {
            device_sample_rate: 44_100.0,
            client_sample_rate: 44_100.0,
            buffer_frames: 512,
            ..observed
        };
        assert_eq!(
            declared_output_frames(subsliced_512, 44_100, 256),
            Some(546)
        );
        let subsliced_1024 = CoreAudioOutputTiming {
            buffer_frames: 1024,
            ..subsliced_512
        };
        assert_eq!(
            declared_output_frames(subsliced_1024, 44_100, 256),
            Some(1058)
        );
        // Non-multiple hardware buffers remain refused.
        let non_multiple = CoreAudioOutputTiming {
            buffer_frames: 384,
            ..subsliced_512
        };
        assert_eq!(declared_output_frames(non_multiple, 44_100, 256), None);
        for invalid in [
            CoreAudioOutputTiming {
                device_sample_rate: 44_100.0,
                ..observed
            },
            CoreAudioOutputTiming {
                client_sample_rate: f64::NAN,
                ..observed
            },
            CoreAudioOutputTiming {
                audio_unit_seconds: 0.004,
                ..observed
            },
            CoreAudioOutputTiming {
                audio_unit_seconds: f64::NAN,
                ..observed
            },
            CoreAudioOutputTiming {
                stream_frames: u32::MAX,
                ..observed
            },
            CoreAudioOutputTiming {
                device_id: 0,
                ..observed
            },
        ] {
            assert_eq!(declared_output_frames(invalid, 48_000, 128), None);
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn headless_output_cannot_claim_hardware_timing() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (output, _backend) = super::super::headless_worker_output(48_000).unwrap();
        assert!(output.observe_plugin_output_on_control().is_none());
    }
}
