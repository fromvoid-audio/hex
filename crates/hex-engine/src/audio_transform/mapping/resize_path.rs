//! Research prototype for ordered, certificate-driven drag prefixes.
//!
//! Kept test-only until the mapping certificate and worst-shape work bounds are
//! qualified. In particular, an uncertain certificate is not a shipping promise
//! to enumerate every frame. Only all-valid certificates may skip an interval.

mod constant_root;
mod rounded;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Work {
    certificates: u64,
    singletons: u64,
}

#[derive(Clone, Copy, Debug)]
struct Frontier {
    left: u64,
    right: u64,
    left_blocked: bool,
    right_blocked: bool,
}

impl Frontier {
    // The caller must have established exact canonical admission at `accepted`.
    fn new(accepted: u64) -> Self {
        Self {
            left: accepted,
            right: accepted,
            left_blocked: false,
            right_blocked: false,
        }
    }

    fn request<E>(
        &mut self,
        requested: u64,
        certificate: &mut impl FnMut(u64, u64) -> Result<bool, E>,
        singleton: &mut impl FnMut(u64) -> Result<bool, E>,
        work: &mut Work,
    ) -> Result<u64, E> {
        if (self.left..=self.right).contains(&requested) {
            return Ok(requested);
        }
        let rightward = requested > self.right;
        let (known, blocked) = if rightward {
            (self.right, self.right_blocked)
        } else {
            (self.left, self.left_blocked)
        };
        if blocked {
            return Ok(known);
        }
        let (low, high) = if rightward {
            (known + 1, requested)
        } else {
            (requested, known - 1)
        };
        // Update custody only after successful proof. An arithmetic/allocation
        // error cannot publish a partly proved frontier or a guessed boundary.
        let first_bad = first_invalid(low, high, rightward, certificate, singleton, work)?;
        let accepted = first_bad.map_or(requested, |bad| if rightward { bad - 1 } else { bad + 1 });
        if rightward {
            self.right = accepted;
            self.right_blocked = first_bad.is_some();
        } else {
            self.left = accepted;
            self.left_blocked = first_bad.is_some();
        }
        Ok(accepted)
    }
}

fn first_invalid<E>(
    low: u64,
    high: u64,
    rightward: bool,
    certificate: &mut impl FnMut(u64, u64) -> Result<bool, E>,
    singleton: &mut impl FnMut(u64) -> Result<bool, E>,
    work: &mut Work,
) -> Result<Option<u64>, E> {
    if low == high {
        work.singletons += 1;
        return Ok((!singleton(low)?).then_some(low));
    }
    work.certificates += 1;
    if certificate(low, high)? {
        return Ok(None);
    }
    let middle = low + (high - low) / 2;
    let (near, far) = if rightward {
        ((low, middle), (middle + 1, high))
    } else {
        ((middle + 1, high), (low, middle))
    };
    if let Some(bad) = first_invalid(near.0, near.1, rightward, certificate, singleton, work)? {
        return Ok(Some(bad));
    }
    first_invalid(far.0, far.1, rightward, certificate, singleton, work)
}

#[test]
fn exhaustive_small_masks_preserve_first_boundary_and_reversal() {
    // Every disconnected validity topology on twelve integer endpoints. The
    // certificate here is an independent exhaustive test oracle, not the future
    // production mapping certificate. Test both no proofs and sound proofs.
    for mask in 1_u16..1 << 12 {
        let valid = |frame: u64| mask & (1 << frame) != 0;
        for origin in 0..12 {
            if !valid(origin) {
                continue;
            }
            let expected_left = (0..origin).rev().find(|&f| !valid(f)).map_or(0, |f| f + 1);
            let expected_right = (origin + 1..12).find(|&f| !valid(f)).map_or(11, |f| f - 1);
            for certify in [false, true] {
                let mut frontier = Frontier::new(origin);
                let mut work = Work::default();
                for request in [11, 0, origin, 7, 2, 11, 0] {
                    let result = frontier
                        .request(
                            request,
                            &mut |lo, hi| Ok::<_, ()>(certify && (lo..=hi).all(valid)),
                            &mut |frame| Ok(valid(frame)),
                            &mut work,
                        )
                        .unwrap();
                    assert_eq!(
                        result,
                        request.clamp(expected_left, expected_right),
                        "mask={mask:x} origin={origin} request={request}"
                    );
                }
                let before = work;
                for request in expected_left..=expected_right {
                    assert_eq!(
                        frontier.request(
                            request,
                            &mut |_, _| Ok::<_, ()>(false),
                            &mut |_| Ok(false),
                            &mut work
                        ),
                        Ok(request)
                    );
                }
                assert_eq!(
                    work, before,
                    "proven reversal must perform no admission work"
                );
            }
        }
    }
}

#[test]
fn large_sound_certificate_skips_only_requested_prefix_and_errors_are_atomic() {
    let mut frontier = Frontier::new(1_000);
    let mut work = Work::default();
    assert_eq!(
        frontier.request(
            16_000_000,
            &mut |lo, hi| {
                assert_eq!((lo, hi), (1_001, 16_000_000));
                Ok::<_, ()>(true)
            },
            &mut |_| panic!("certified interval needs no singleton"),
            &mut work
        ),
        Ok(16_000_000)
    );
    assert_eq!(
        work,
        Work {
            certificates: 1,
            singletons: 0
        }
    );
    assert_eq!(
        frontier.left, 1_000,
        "unused opposite direction remains uncomputed"
    );
    let before = frontier;
    assert_eq!(
        frontier.request(
            0,
            &mut |_, _| Err("research abort"),
            &mut |_| Ok(true),
            &mut work
        ),
        Err("research abort")
    );
    assert_eq!(
        (
            frontier.left,
            frontier.right,
            frontier.left_blocked,
            frontier.right_blocked
        ),
        (
            before.left,
            before.right,
            before.left_blocked,
            before.right_blocked
        )
    );
}

#[test]
fn canonical_small_warp_paths_match_exhaustive_admission() {
    use hex_project::{Command, Created, Editor, Project, TrimEdge, preview_audio_clip_stretch};
    for pitch in [-1_200, 0, 1_200, 2_400] {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "small".into(),
                path: "small.wav".into(),
                sample_rate: 16_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: pitch })
            .unwrap();
        let original = editor.project().clip(clip).unwrap();
        let tempo = editor.project().tempo_map();
        let admitted = |frame| {
            preview_audio_clip_stretch(original, TrimEdge::Right, frame, tempo, 32)
                .and_then(|candidate| {
                    super::super::validate_clip_warp_preview(&candidate, tempo)
                        .map_err(|error| hex_project::Invalid(error.0))
                })
                .is_ok()
        };
        let truth: Vec<bool> = (0..=96).map(admitted).collect();
        assert!(truth[32]);
        let left = (0..32).rev().find(|&f| !truth[f]).map_or(0, |f| f + 1);
        let right = (33..=96).find(|&f| !truth[f]).map_or(96, |f| f - 1);
        // No interval certificate yet: deliberately bounded small-domain oracle
        // exercise. This is not a shipping frame-enumeration fallback.
        let mut frontier = Frontier::new(32);
        let mut work = Work::default();
        for request in (0..=96).rev().chain(0..=96) {
            let result = frontier
                .request(
                    request,
                    &mut |_, _| Ok::<_, ()>(false),
                    &mut |f| Ok(admitted(f)),
                    &mut work,
                )
                .unwrap();
            assert_eq!(
                result,
                request.clamp(left as u64, right as u64),
                "pitch={pitch} request={request}"
            );
        }
        eprintln!("canonical path pitch={pitch}: connected=[{left},{right}] work={work:?}");
    }
}
