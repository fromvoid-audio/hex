//! Projection for each member of an already validated grouped take. Reuse the
//! same clock and span interpreter as the ordinary single-target file. The
//! complete grouped source remains locked; bounded temporary rows stay on control.
use super::*;
use crate::automation_draft::{
    CompletedGroupedDraft, DraftTarget, GroupApplication, GroupDraftEvent, MAX_GROUP_TARGETS,
};
use hex_project::DeviceAutomationTarget;

struct MemberReader<'a> {
    source: &'a Project,
    target: DraftTarget,
    provenance: DraftProvenance,
    capture: CaptureClock,
    end: DraftClock,
    identity: AutomationContentIdentity,
    path: &'a Path,
    values: Vec<AppliedEvent>,
}

impl TakeReader for MemberReader<'_> {
    fn captured_tempo_map(&self) -> &TempoMap {
        self.source.tempo_map()
    }
    fn target(&self) -> DraftTarget {
        self.target
    }
    fn provenance(&self) -> DraftProvenance {
        self.provenance
    }
    fn capture_clock(&self) -> Option<CaptureClock> {
        Some(self.capture)
    }
    fn end_clock(&self) -> DraftClock {
        self.end
    }
    fn content_identity(&self) -> AutomationContentIdentity {
        self.identity
    }
    fn event_count(&self) -> u64 {
        self.values.len() as u64
    }
    fn path(&self) -> &Path {
        self.path
    }
    fn read_events(
        &mut self,
        first: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError> {
        output.fill(None);
        let tail = usize::try_from(first)
            .ok()
            .and_then(|first| self.values.get(first..))
            .ok_or_else(|| DraftError {
                path: self.path.to_path_buf(),
                progress: super::super::DraftProgress::default(),
                published: true,
                source: Box::new(crate::StorageError::Invalid(
                    "grouped take member read exceeds its validated extent",
                )),
            })?;
        let count = tail.len().min(output.len());
        for (&value, slot) in tail.iter().zip(output.iter_mut()).take(count) {
            *slot = Some(value);
        }
        Ok(count)
    }
    fn event_with_ordinal_at_or_before(
        &mut self,
        performance: u64,
    ) -> Result<Option<(u64, AppliedEvent)>, DraftError> {
        let boundary = self
            .values
            .partition_point(|event| event.performance_frame <= performance);
        Ok(boundary
            .checked_sub(1)
            .map(|index| (index as u64, self.values[index])))
    }
}

/// A bounded read-only view of one member's exact value-to-restoration span.
pub struct ProjectedGroupLane<'a> {
    parameter: u32,
    core: TakeCore<MemberReader<'a>>,
}

impl ProjectedGroupLane<'_> {
    pub fn parameter(&self) -> u32 {
        self.parameter
    }
    pub fn projected_coverage(
        &self,
        destination: &TempoMap,
    ) -> Result<[Option<Range<u64>>; 2], String> {
        self.core.projected_coverage(destination)
    }
    pub fn begin_projected_spans_on_control(
        &mut self,
        destination: Arc<TempoMap>,
        range: Range<u64>,
    ) -> Result<ProjectedSpanCursor, String> {
        self.core
            .begin_projected_spans_on_control(destination, range)
    }
    pub fn read_projected_spans_on_control(
        &mut self,
        destination: &Arc<TempoMap>,
        cursor: &mut ProjectedSpanCursor,
        input_budget: usize,
        output: &mut [Option<ProjectedAutomationSpan>],
    ) -> Result<SpanReadProgress, String> {
        self.core
            .read_projected_spans_on_control(destination, cursor, input_budget, output)
    }
}

impl CompletedGroupedDraft {
    /// Project complete group members on control. The caller supplies a hard
    /// work bound; storage never indexes an unbounded or partially restored take.
    pub fn projected_lanes_on_control(
        &mut self,
        max_events: u64,
    ) -> Result<Vec<ProjectedGroupLane<'_>>, String> {
        const MAX_GROUP_PROJECTION_RECORDS: u64 = 65_536;
        if self.event_count() > max_events.min(MAX_GROUP_PROJECTION_RECORDS) {
            return Err(
                "sealed group exceeds the bounded projection work; retain the draft".into(),
            );
        }
        let target = self.target();
        let capture = self
            .capture_clock()
            .ok_or("sealed group has no capture clock")?;
        let mut values: [Vec<AppliedEvent>; MAX_GROUP_TARGETS] =
            std::array::from_fn(|_| Vec::new());
        let mut ends = [None; MAX_GROUP_TARGETS];
        let mut cursor = 0;
        let mut page: [Option<GroupDraftEvent>; MAX_APPEND_EVENTS] = [None; MAX_APPEND_EVENTS];
        while cursor < self.event_count() {
            let count = self
                .read_events(cursor, &mut page)
                .map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("sealed group lost its validated prefix".into());
            }
            for row in page[..count].iter().flatten() {
                let index = target
                    .ids()
                    .binary_search(&row.parameter)
                    .map_err(|_| "sealed group target changed")?;
                match row.application {
                    GroupApplication::Value(value) => {
                        let value = hex_project::AutomationValue::new(value as f32)
                            .map_err(|e| e.to_string())?;
                        values[index]
                            .try_reserve(1)
                            .map_err(|_| "could not reserve grouped projection")?;
                        values[index].push(AppliedEvent {
                            sequence: row.ordinal,
                            performance_frame: row.clock.performance_frame,
                            content_frame: row.clock.content_frame,
                            loop_pass: row.clock.loop_pass,
                            value,
                        });
                    }
                    GroupApplication::Restored => ends[index] = Some(row.clock),
                }
            }
            cursor += count as u64;
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(target.ids().len())
            .map_err(|_| "could not reserve grouped projection lanes")?;
        for ((&parameter, values), end) in target.ids().iter().zip(values).zip(ends) {
            let first = *values.first().ok_or("sealed group member has no value")?;
            let end = end.ok_or("sealed group member has no restoration")?;
            let mapping = ClockMapping::new(first, end, capture.loop_region)?;
            output.push(ProjectedGroupLane {
                parameter,
                core: TakeCore {
                    source: MemberReader {
                        source: self.source(),
                        target: DraftTarget::Device {
                            device: target.device(),
                            target: DeviceAutomationTarget::Vst3 { parameter },
                        },
                        provenance: self.provenance(),
                        capture,
                        end,
                        identity: self.content_identity(),
                        path: self.path(),
                        values,
                    },
                    mapping: Some(mapping),
                    cursor_owner: Arc::new(()),
                },
            });
        }
        Ok(output)
    }
}
