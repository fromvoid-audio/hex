use super::*;
use crate::foreign_automation::{MAX_POINTS, Program, prepare_knots};
use hex_project::{
    AutomationInterpolation as Interpolation, AutomationPoint, AutomationValue, BeatTime,
    DeviceAutomationTouchStep, Tempo, TempoPoint,
};

fn tempo(changing: bool) -> TempoMap {
    let mut points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(120_000_000).unwrap(),
    )];
    if changing {
        points.push(TempoPoint::new(
            BeatTime::ONE,
            Tempo::new(60_000_000).unwrap(),
        ));
    }
    TempoMap::new(8192, points).unwrap()
}

fn span(map: &TempoMap, start: u64, end: u64, values: &[(u64, f32)]) -> DeviceAutomationTouchSpan {
    DeviceAutomationTouchSpan {
        start: map.frame_to_beat(start).unwrap(),
        end: map.frame_to_beat(end).unwrap(),
        steps: values
            .iter()
            .map(|&(frame, value)| DeviceAutomationTouchStep {
                beat: map.frame_to_beat(frame).unwrap(),
                value: AutomationValue::new(value).unwrap(),
            })
            .collect(),
    }
}

fn program(
    map: &TempoMap,
    source: &[DeviceAutomationTouchSpan],
    stepped: u32,
    frames: u16,
    canonical: Option<f64>,
) -> Program {
    let knots = canonical.map_or_else(
        || {
            prepare_knots(
                &[
                    AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.1).unwrap(),
                        outgoing: Interpolation::Linear,
                    },
                    AutomationPoint {
                        beat: BeatTime::from_quarters(2).unwrap(),
                        value: AutomationValue::new(0.9).unwrap(),
                        outgoing: Interpolation::Step,
                    },
                ],
                stepped,
                map,
            )
            .unwrap()
        },
        |value| {
            vec![Knot {
                frame: 0,
                value,
                linear: false,
            }]
        },
    );
    let overlay = Overlay::prepare(source, &knots, stepped, map).unwrap();
    let profile = QuantumProfile {
        frames,
        max_runs: frames.min(3),
    };
    let point_budget = overlay.as_ref().unwrap().point_bound(profile);
    assert!(point_budget <= MAX_POINTS);
    Program {
        lanes: vec![Lane {
            id: 0,
            knots,
            overlay,
        }],
        profile,
        point_budget,
    }
}

fn rendered(
    program: &Program,
    origins: &[Option<u64>],
    cuts: &[u16],
) -> Vec<ForeignParameterPoint> {
    let mut output = [ForeignParameterPoint::default(); MAX_POINTS];
    let written = program.render_quantum(origins, cuts, &mut output).unwrap();
    output[..written].to_vec()
}

// Independent SDK-style linear interpolation between delivered queue points.
fn sample(points: &[ForeignParameterPoint], offset: u16) -> f64 {
    let next = points.partition_point(|point| point.frame <= offset);
    let left = points[next - 1];
    let Some(right) = points.get(next) else {
        return left.normalized;
    };
    left.normalized
        + (right.normalized - left.normalized) * f64::from(offset - left.frame)
            / f64::from(right.frame - left.frame)
}

fn base(map: &TempoMap, frame: u64) -> f64 {
    let beat = map.frame_to_beat(frame).unwrap().raw() as f64 / BeatTime::ONE.raw() as f64;
    f64::from(0.1_f32) + (f64::from(0.9_f32) - f64::from(0.1_f32)) * beat / 2.0
}

#[test]
fn untouched_linear_values_and_exclusive_restore_survive_touch_cuts() {
    let map = tempo(false);
    let spans = [span(&map, 16, 48, &[(16, 0.75), (32, 0.25)])];
    let p = program(&map, &spans, 0, 64, None);
    let origins: Vec<_> = (0..64).map(Some).collect();
    let points = rendered(&p, &origins, &[]);
    for frame in 0..64 {
        let expected = match frame {
            16..32 => 0.75,
            32..48 => 0.25,
            _ => base(&map, frame),
        };
        assert!(
            (sample(&points, frame as u16) - expected).abs() < 1e-12,
            "frame{frame}"
        );
    }
    assert_eq!(
        p.lanes[0].knots.len(),
        2,
        "original anchors remain unchanged"
    );
}

#[test]
fn adjacent_stepped_touches_restore_the_exact_canonical_base() {
    let map = tempo(false);
    let p = program(
        &map,
        &[span(&map, 4, 7, &[(4, 0.6)]), span(&map, 7, 9, &[(7, 0.1)])],
        3,
        16,
        Some(1.0 / 3.0),
    );
    let points = rendered(&p, &(0..16).map(Some).collect::<Vec<_>>(), &[]);
    for frame in 0..16 {
        let expected = match frame {
            4..7 => 2.0 / 3.0,
            7..9 => 0.0,
            _ => 1.0 / 3.0,
        };
        assert_eq!(sample(&points, frame), expected);
    }
}

#[test]
fn tempo_cut_loop_seek_and_missing_origin_chases_are_independent_of_quantum_size() {
    let map = tempo(true);
    let spans = [span(&map, 4084, 4110, &[(4084, 0.75), (4100, 0.25)])];
    let p = program(&map, &spans, 0, 16, None);
    let origins: Vec<_> = (4080..4088)
        .map(Some)
        .chain((0..4).map(Some))
        .chain([None, None, Some(4104), Some(4105)])
        .collect();
    let points = rendered(&p, &origins, &[14]);
    assert!(points.iter().all(|point| ![12, 13].contains(&point.frame)));
    for (offset, origin) in origins.iter().enumerate() {
        if let Some(frame) = origin {
            let expected = if (4084..4100).contains(frame) {
                0.75
            } else if (4100..4110).contains(frame) {
                0.25
            } else {
                base(&map, *frame)
            };
            assert!((sample(&points, offset as u16) - expected).abs() < 1e-12);
        }
    }
    let p8 = program(&map, &spans, 0, 8, None);
    let whole = rendered(&p, &(4104..4120).map(Some).collect::<Vec<_>>(), &[]);
    for start in [4104, 4112] {
        let partition = rendered(&p8, &(start..start + 8).map(Some).collect::<Vec<_>>(), &[]);
        for offset in 0..8 {
            assert!(
                (sample(&whole, (start - 4104) as u16 + offset) - sample(&partition, offset)).abs()
                    < 1e-12
            );
        }
    }
}

#[test]
fn malformed_spans_and_excess_density_are_refused_without_changing_the_base() {
    let map = tempo(false);
    let base = [Knot {
        frame: 0,
        value: 0.5,
        linear: false,
    }];
    for spans in [
        vec![span(&map, 4, 4, &[(4, 0.2)])],
        vec![span(&map, 4, 9, &[(5, 0.2)])],
        vec![span(&map, 4, 9, &[(4, 0.2), (4, 0.3)])],
        vec![
            span(&map, 4, 9, &[(4, 0.2)]),
            span(&map, 8, 12, &[(8, 0.3)]),
        ],
    ] {
        assert!(Overlay::prepare(&spans, &base, 0, &map).is_err());
    }
    let values: Vec<_> = (0..128).step_by(2).map(|frame| (frame, 0.25)).collect();
    let overlay = Overlay::prepare(&[span(&map, 0, 128, &values)], &base, 0, &map)
        .unwrap()
        .unwrap();
    assert!(
        overlay.point_bound(QuantumProfile {
            frames: 256,
            max_runs: 9
        }) > MAX_POINTS
    );
    assert_eq!(base[0].value, 0.5);
}

#[test]
fn program_accounts_for_every_retained_overlay_allocation() {
    let map = tempo(false);
    let p = program(
        &map,
        &[
            span(&map, 4, 7, &[(4, 0.6), (6, 0.2)]),
            span(&map, 9, 12, &[(9, 0.1)]),
        ],
        0,
        16,
        Some(0.5),
    );
    let lane = &p.lanes[0];
    let overlay = lane.overlay.as_ref().unwrap();
    let span_bytes = overlay.spans.capacity() * std::mem::size_of::<Span>();
    let cut_bytes = overlay.cuts.capacity() * std::mem::size_of::<u64>();
    let step_bytes: usize = overlay
        .spans
        .iter()
        .map(|span| span.steps.capacity() * std::mem::size_of::<Knot>())
        .sum();
    assert!(span_bytes > 0 && cut_bytes > 0 && step_bytes > 0);
    let all_backing = p.lanes.capacity() * std::mem::size_of::<Lane>()
        + lane.knots.capacity() * std::mem::size_of::<Knot>()
        + span_bytes
        + cut_bytes
        + step_bytes;
    assert_eq!(p.backing_bytes(), Some(all_backing));
}
