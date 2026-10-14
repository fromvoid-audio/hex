//! Shared bounded test sink for the owned-container coverage fixtures.
use super::*;
pub(crate) struct Sink<'a> {
    pub(crate) items: Vec<StorageItem<'a>>,
    limit: usize,
}
impl<'a> Sink<'a> {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            items: Vec::with_capacity(limit),
            limit,
        }
    }
    pub(crate) fn measured(&self, name: &str, indices: [usize; 2]) -> (usize, usize, usize) {
        let item = self
            .items
            .iter()
            .find(|i| i.location.component == name && i.location.indices == indices)
            .unwrap();
        let StorageCharge::MeasuredData {
            capacity,
            element_bytes,
            bytes,
        } = item.charge
        else {
            panic!("measured backing")
        };
        (capacity, element_bytes, bytes)
    }
}
impl<'a> PlaybackStorageVisitor<'a> for Sink<'a> {
    fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
        if self.items.len() == self.limit {
            return Err(StorageReportError::SinkCapacity);
        }
        self.items.push(item);
        Ok(())
    }
}
#[test]
fn owned_capacity_overflow_refuses_before_emission() {
    let mut sink = Sink::new(1);
    let mut report = Report::new(&mut sink);
    assert_eq!(
        report.data(StorageOwner::Exclusive, "backing", [0; 2], usize::MAX, 2),
        Err(StorageReportError::ArithmeticOverflow)
    );
    assert!(sink.items.is_empty());
}
