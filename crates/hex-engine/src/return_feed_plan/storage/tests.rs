use super::*;
use crate::storage_report::{SharedOwnerRef, StorageCharge, StorageItem, UnknownStorageReason};
use std::sync::Arc;
struct Sink<'a> {
    items: [Option<StorageItem<'a>>; 32],
    len: usize,
    refuse_at: Option<usize>,
    strong: (&'a Arc<ReturnFeedPlan>, usize),
}
impl<'a> PlaybackStorageVisitor<'a> for Sink<'a> {
    fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
        assert_eq!(Arc::strong_count(self.strong.0), self.strong.1);
        if self.refuse_at == Some(self.len) || self.len == self.items.len() {
            return Err(StorageReportError::SinkCapacity);
        }
        self.items[self.len] = Some(item);
        self.len += 1;
        Ok(())
    }
}
fn fixture() -> Arc<ReturnFeedPlan> {
    let mut order = Vec::with_capacity(17);
    order.extend([0, 1, 2]);
    let mut outgoing = Vec::with_capacity(11);
    let mut first = Vec::with_capacity(13);
    first.push(Feed {
        destination: 1,
        gain: 0.5,
        id: 1,
    });
    first.push(Feed {
        destination: 2,
        gain: 0.25,
        id: 2,
    });
    outgoing.push(first);
    outgoing.push(Vec::with_capacity(7));
    outgoing.push(Vec::with_capacity(9));
    Arc::new(ReturnFeedPlan { order, outgoing })
}
fn sink(root: &Arc<ReturnFeedPlan>, refuse_at: Option<usize>) -> Sink<'_> {
    Sink {
        items: [None; 32],
        len: 0,
        refuse_at,
        strong: (root, Arc::strong_count(root)),
    }
}
fn owner(root: &Arc<ReturnFeedPlan>) -> StorageOwner<'_> {
    StorageOwner::Shared(SharedOwnerRef::new(root, "test_return_feed_plan"))
}
fn total(sink: &Sink<'_>) -> usize {
    sink.items[..sink.len]
        .iter()
        .flatten()
        .map(|item| match item.charge {
            StorageCharge::MeasuredData { bytes, .. } => bytes,
            _ => panic!("family has no provider/overhead/unknown costs"),
        })
        .sum()
}

fn deduplicated_total(sink: &Sink<'_>) -> usize {
    sink.items[..sink.len]
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let item = item.as_ref().unwrap();
            let StorageOwner::Shared(owner) = item.owner else {
                panic!("shared root")
            };
            let repeated = sink.items[..index].iter().flatten().any(|previous| {
                matches!(previous.owner, StorageOwner::Shared(old) if owner.same_allocation(old))
                    && previous.location.component == item.location.component
                    && previous.location.indices == item.location.indices
            });
            if repeated {
                None
            } else {
                let StorageCharge::MeasuredData { bytes, .. } = item.charge else {
                    panic!("measured")
                };
                Some(bytes)
            }
        })
        .sum()
}

#[test]
fn all_nested_capacities_are_charged_once_including_empty_buffers() {
    let root = fixture();
    let mut sink = sink(&root, None);
    let mut report = Report::new(&mut sink);
    root.visit_storage(&mut report, owner(&root)).unwrap();
    assert_eq!(report.finish().require_complete(), Ok(()));
    let expected = root.order.capacity() * std::mem::size_of::<usize>()
        + root.outgoing.capacity() * std::mem::size_of::<Vec<Feed>>()
        + root
            .outgoing
            .iter()
            .map(|v| v.capacity() * std::mem::size_of::<Feed>())
            .sum::<usize>();
    assert_eq!(total(&sink), expected);
    assert_eq!(sink.len, 5);
    let item = sink.items[3].unwrap();
    assert_eq!(item.location.component, "return_feed.feeds");
    assert_eq!(item.location.indices, [1, 0]);
    assert!(matches!(
        item.charge,
        StorageCharge::MeasuredData { capacity: 7, .. }
    ));
    // Positive controls reject length-only and repeated inline-header charges.
    assert!(
        expected
            > root.order.len() * std::mem::size_of::<usize>()
                + root.outgoing.len() * std::mem::size_of::<Vec<Feed>>()
                + 2 * std::mem::size_of::<Feed>()
    );
    assert_ne!(
        total(&sink),
        expected + std::mem::size_of::<ReturnFeedPlan>()
    );
}
#[test]
fn repeat_shared_root_deduplicates_each_container_but_equal_plan_is_distinct() {
    let root = fixture();
    let same = root.clone();
    let distinct = Arc::new(root.as_ref().clone());
    let mut sink = sink(&root, None);
    let mut report = Report::new(&mut sink);
    for plan in [&root, &same, &distinct] {
        plan.visit_storage(&mut report, owner(plan)).unwrap();
    }
    report.finish().require_complete().unwrap();
    let distinct_bytes = sink.items[10..15]
        .iter()
        .flatten()
        .map(|item| {
            let StorageCharge::MeasuredData { bytes, .. } = item.charge else {
                panic!("data")
            };
            bytes
        })
        .sum::<usize>();
    let original_bytes = sink.items[..5]
        .iter()
        .flatten()
        .map(|item| {
            let StorageCharge::MeasuredData { bytes, .. } = item.charge else {
                panic!("data")
            };
            bytes
        })
        .sum::<usize>();
    assert_eq!(deduplicated_total(&sink), original_bytes + distinct_bytes);
    assert_eq!(total(&sink), 2 * original_bytes + distinct_bytes);
    for index in 0..5 {
        let a = sink.items[index].unwrap();
        let b = sink.items[index + 5].unwrap();
        let c = sink.items[index + 10].unwrap();
        let (
            StorageOwner::Shared(a_owner),
            StorageOwner::Shared(b_owner),
            StorageOwner::Shared(c_owner),
        ) = (a.owner, b.owner, c.owner)
        else {
            panic!("actual roots")
        };
        assert!(a_owner.same_allocation(b_owner));
        assert!(!a_owner.same_allocation(c_owner));
        assert_eq!(a.location.component, b.location.component);
        assert_eq!(a.location.indices, b.location.indices);
    }
}
#[test]
fn every_sink_refusal_preserves_buffers_and_routing_and_overflow_refuses() {
    let root = fixture();
    let pointers = (
        root.order.as_ptr(),
        root.outgoing.as_ptr(),
        root.outgoing[0].as_ptr(),
    );
    for cut in 0..5 {
        let mut sink = sink(&root, Some(cut));
        let mut report = Report::new(&mut sink);
        assert_eq!(
            root.visit_storage(&mut report, owner(&root)),
            Err(StorageReportError::SinkCapacity)
        );
        assert_eq!(
            pointers,
            (
                root.order.as_ptr(),
                root.outgoing.as_ptr(),
                root.outgoing[0].as_ptr()
            )
        );
    }
    let mut sink = sink(&root, None);
    let mut report = Report::new(&mut sink);
    assert_eq!(
        report.data(
            owner(&root),
            "return_feed.feeds",
            [0; 2],
            usize::MAX,
            std::mem::size_of::<Feed>()
        ),
        Err(StorageReportError::ArithmeticOverflow)
    );
    root.visit_storage(&mut report, owner(&root)).unwrap();
    report.finish().require_complete().unwrap();
    let mut scratch = vec![[[0.0; 2]; 256]; 3];
    scratch[0].fill([1.0, -1.0]);
    for &index in root.order() {
        root.propagate(index, &mut scratch, 256);
    }
    assert_eq!(scratch[1][255], [0.5, -0.5]);
    assert_eq!(scratch[2][255], [0.25, -0.25]);
}
#[test]
fn complete_family_does_not_claim_arc_or_playback_completeness() {
    let root = fixture();
    let mut sink = sink(&root, None);
    let mut report = Report::new(&mut sink);
    let rooted_owner = report.arc(&root, "test_return_feed_plan").unwrap();
    root.visit_storage(&mut report, rooted_owner).unwrap();
    assert!(report.finish().require_complete().is_err());
    assert!(sink.items.iter().flatten().any(|item| matches!(
        item.charge,
        StorageCharge::Unknown(UnknownStorageReason::ArcOwnerOverhead)
    )));
}
#[test]
fn empty_plan_does_not_recharge_inline_headers_or_invent_backing_storage() {
    let root = Arc::new(ReturnFeedPlan::default());
    let mut sink = sink(&root, None);
    let mut report = Report::new(&mut sink);
    root.visit_storage(&mut report, owner(&root)).unwrap();
    report.finish().require_complete().unwrap();
    assert_eq!(sink.len, 2);
    assert_eq!(total(&sink), 0);
}
#[test]
fn public_playback_visitor_reaches_all_return_feed_storage_and_keeps_other_unknowns() {
    let fixture = fixture();
    let mut graph = crate::Project::new(48_000, Vec::new()).unwrap();
    graph.return_feed_plan = fixture.as_ref().clone();
    let plan = &graph.return_feed_plan;
    let expected = plan.order.capacity() * std::mem::size_of::<usize>()
        + plan.outgoing.capacity() * std::mem::size_of::<Vec<Feed>>()
        + plan
            .outgoing
            .iter()
            .map(|v| v.capacity() * std::mem::size_of::<Feed>())
            .sum::<usize>();
    let mut playback = crate::playback::Playback::new(graph);
    let mut sink = sink(&fixture, None);
    let summary = playback.visit_storage_on_control(&mut sink).unwrap();
    let mut components = 0;
    let mut bytes = 0;
    for item in sink.items[..sink.len].iter().flatten() {
        if item.location.component.starts_with("return_feed.") {
            let StorageCharge::MeasuredData {
                bytes: measured, ..
            } = item.charge
            else {
                panic!("actual production seam must report known data")
            };
            bytes += measured;
            components += 1;
        }
        assert_ne!(item.location.component, "arrangement.return_feed_plan");
    }
    assert_eq!(components, 5);
    assert_eq!(bytes, expected);
    assert!(summary.require_complete().is_err());
}
