use mts_esp_master_adapter::{Error, Master, TuningPublication};

pub(super) trait Transport {
    fn acquire(&mut self, recover: bool) -> Result<(), Error>;
    fn release(&mut self);
    fn publish(&mut self, publication: &TuningPublication);
    fn clients(&self) -> Result<u32, Error>;
}

#[derive(Default)]
pub(super) struct Native {
    master: Option<Master>,
}
impl Transport for Native {
    fn acquire(&mut self, recover: bool) -> Result<(), Error> {
        self.master = Some(if recover {
            Master::recover()?
        } else {
            Master::register()?
        });
        Ok(())
    }
    fn release(&mut self) {
        self.master = None;
    }
    fn publish(&mut self, publication: &TuningPublication) {
        if let Some(master) = &self.master {
            master.publish(publication);
        }
    }
    fn clients(&self) -> Result<u32, Error> {
        self.master
            .as_ref()
            .ok_or(Error::LibraryUnavailable)?
            .client_count()
            .map(|count| count.get())
    }
}
