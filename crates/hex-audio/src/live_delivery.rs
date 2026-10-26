//! Fixed control credits and Copy delivery markers shared by live endpoints.
//! Terminal commands reserve two independent slots beyond normal in-flight work.
pub(crate) const IN_FLIGHT: usize = 64;
pub(crate) const CAPACITY: usize = IN_FLIGHT + 2;
#[derive(Default)]
pub(crate) struct Credits(usize);
impl Credits {
    pub(crate) fn available(&self) -> bool {
        self.0 < IN_FLIGHT
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0 == 0
    }
    pub(crate) fn issued(&mut self) {
        self.0 += 1;
    }
    pub(crate) fn retired(&mut self) {
        self.0 = self.0.saturating_sub(1);
    }
}
pub(crate) struct Pending<R: Copy> {
    slots: [Option<(u64, R)>; CAPACITY],
    len: usize,
}
impl<R: Copy> Default for Pending<R> {
    fn default() -> Self {
        Self {
            slots: [None; CAPACITY],
            len: 0,
        }
    }
}
impl<R: Copy> Pending<R> {
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub(crate) fn push(&mut self, due: u64, receipt: R) -> Result<(), R> {
        let Some(slot) = self.slots.get_mut(self.len) else {
            return Err(receipt);
        };
        *slot = Some((due, receipt));
        self.len += 1;
        Ok(())
    }
    pub(crate) fn pop_due(&mut self, end: u64) -> Option<R> {
        if self.slots.first()?.as_ref()?.0 >= end {
            return None;
        }
        self.pop()
    }
    pub(crate) fn pop(&mut self) -> Option<R> {
        if self.len == 0 {
            return None;
        }
        let (_, receipt) = self.slots[0].take()?;
        self.slots.copy_within(1..self.len, 0);
        self.len -= 1;
        self.slots[self.len] = None;
        Some(receipt)
    }
}
