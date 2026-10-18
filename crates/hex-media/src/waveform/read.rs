//! Scoped renderer queries without access to owning waveform/PCM children.
use super::{Peak, SharedWaveform, Waveform};
use hex_engine::SharedMetadataCacheIdentity;

/// A borrowed immutable waveform query surface. Copies remain lifetime-bound;
/// no raw waveform, shared owner, PCM buffer or source lease can be extracted.
/// This is a read view, not a snapshot capability or lifetime/admission permit.
///
/// ```compile_fail
/// fn escape(read: hex_media::waveform::WaveformRead<'_>) -> &hex_media::waveform::Waveform {
///     read.as_ref()
/// }
/// ```
/// ```compile_fail
/// fn escape(source: &hex_media::waveform::SharedWaveform)
///     -> hex_media::waveform::WaveformRead<'static>
/// {
///     hex_media::waveform::read_shared(source)
/// }
/// ```
/// ```compile_fail
/// fn escape(read: hex_media::waveform::WaveformRead<'_>) {
///     let _owner = read.original_source().unwrap().clone();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct WaveformRead<'a> {
    waveform: &'a Waveform,
    version: Option<SharedMetadataCacheIdentity>,
}
impl WaveformRead<'_> {
    pub fn source_frames(self) -> usize {
        self.waveform.source_frames
    }
    /// Total retained bins across the leaf and every pyramid resolution.
    pub fn cached_bin_count(self) -> usize {
        self.waveform.cached_bin_count()
    }
    /// Exact existing PCM/pyramid query, returning scalar peaks by value.
    pub fn peaks_for_frames(self, start: usize, end: usize) -> Option<[Peak; 2]> {
        self.waveform.peaks_for_frames(start, end)
    }
    /// Geometry reuse only, never storage identity or a source lease.
    pub fn cache_identity(self) -> Option<SharedMetadataCacheIdentity> {
        self.version
    }
}
/// Borrow the actual immutable paired waveform without cloning any owner.
/// A sealed snapshot reader may return this only within that snapshot's scope.
pub fn read_shared(source: &SharedWaveform) -> WaveformRead<'_> {
    WaveformRead {
        waveform: source.as_ref(),
        version: source.cache_identity(),
    }
}
impl Waveform {
    /// Raw owned waveform values can be queried without acquiring any shared
    /// identity. This never upgrades an untracked PCM or an unpaired value.
    pub fn read(&self) -> WaveformRead<'_> {
        WaveformRead {
            waveform: self,
            version: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_reads_preserve_exact_queries_and_same_owner_cache_versions() {
        let peaks = vec![
            [
                Peak {
                    min: -0.8,
                    max: 0.25,
                },
                Peak {
                    min: -0.1,
                    max: 0.9,
                },
            ],
            [
                Peak {
                    min: -0.2,
                    max: 0.7,
                },
                Peak {
                    min: -0.6,
                    max: 0.3,
                },
            ],
            [
                Peak {
                    min: -0.4,
                    max: 0.1,
                },
                Peak {
                    min: -0.5,
                    max: 0.2,
                },
            ],
        ];
        let raw = Waveform::from_peaks(17, peaks.clone()).unwrap();
        assert_eq!(raw.read().cache_identity(), None);
        let owner = SharedWaveform::new_on_control(raw);
        let alias = owner.clone();
        let equal = SharedWaveform::new_on_control(Waveform::from_peaks(17, peaks).unwrap());
        let read = read_shared(&owner);
        assert_eq!(read.source_frames(), 17);
        // Existing accounting counts every resolution, not just leaf bins:
        // three leaves produce a two-bin level and a one-bin root.
        assert_eq!(owner.bins.len(), 3);
        assert_eq!(
            owner.levels.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(read.cached_bin_count(), 3 + 2 + 1);
        assert_eq!(read.cache_identity(), read_shared(&alias).cache_identity());
        assert_ne!(read.cache_identity(), read_shared(&equal).cache_identity());
        for start in 0..=18 {
            for end in 0..=18 {
                assert_eq!(
                    read.peaks_for_frames(start, end),
                    owner.peaks_for_frames(start, end)
                );
            }
        }
        assert_eq!(
            read.peaks_for_frames(0, 17),
            Some([
                Peak {
                    min: -0.8,
                    max: 0.7
                },
                Peak {
                    min: -0.6,
                    max: 0.9
                },
            ])
        );
        assert_eq!(read.peaks_for_frames(usize::MAX, usize::MAX), None);
    }
}
