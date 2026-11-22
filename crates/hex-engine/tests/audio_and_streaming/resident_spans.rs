use hex_engine::{Audio, Clip, Frame, Project, Renderer, Track};

#[derive(Clone, Copy)]
struct Region {
    start: u64,
    duration: u64,
    offset: usize,
    length: usize,
    phase: usize,
    looping: bool,
    gain: f32,
}

#[test]
fn neutral_regions_keep_exact_phase_tail_and_mix_order_at_block_and_seek_boundaries() {
    let source: Vec<Frame> = (0..32)
        .map(|i| [(i as f32 - 8.0) / 17.0, -(i as f32 - 6.0) / 31.0])
        .collect();
    let regions = [
        Region {
            start: 9,
            duration: 101,
            offset: 5,
            length: 7,
            phase: 3,
            looping: true,
            gain: 0.7,
        },
        Region {
            start: 11,
            duration: 40,
            offset: 13,
            length: 5,
            phase: 0,
            looping: false,
            gain: 0.7,
        },
        Region {
            start: 0,
            duration: 130,
            offset: 3,
            length: 11,
            phase: 10,
            looping: true,
            gain: 0.3,
        },
    ];
    let audio = Audio::new(48_000, source.clone()).unwrap();
    let clips: Vec<_> = regions
        .iter()
        .map(|r| {
            Clip::region(
                audio.clone(),
                r.start,
                r.duration,
                r.offset as u64,
                r.length as u64,
                r.looping,
            )
            .unwrap()
            .with_source_phase(r.phase as u64)
            .unwrap()
        })
        .collect();
    let project = Project::new(
        48_000,
        vec![
            Track::new(clips[..2].to_vec(), 0.7, false).unwrap(),
            Track::new(vec![clips[2].clone()], 0.3, false).unwrap(),
        ],
    )
    .unwrap();
    for count in [1, 3, 7, 64, 128] {
        for position in [0, 8, 9, 10, 14, 97, 109, 128] {
            let mut renderer = Renderer::new(&project);
            renderer.play();
            renderer.seek(position);
            let mut actual = vec![[0.0; 2]; count];
            renderer.render(&mut actual).unwrap();
            for (i, actual) in actual.iter().enumerate() {
                let frame = position + i as u64;
                let mut expected = [0.0_f32; 2];
                // Independent scalar source-domain definition, in authored mix
                // order. Compare bits, including signed zero, rather than a
                // tolerance that could conceal changed floating-point order.
                for r in regions {
                    if frame < r.start || frame >= r.start + r.duration {
                        continue;
                    }
                    let relative = (frame - r.start) as usize;
                    if !r.looping && relative >= r.length {
                        continue;
                    }
                    let index = r.offset + (relative + r.phase) % r.length;
                    for channel in 0..2 {
                        expected[channel] += source[index][channel] * r.gain;
                    }
                }
                assert_eq!(
                    actual.map(f32::to_bits),
                    expected.map(f32::to_bits),
                    "position {position}, block {count}, frame {i}"
                );
            }
        }
    }
}
