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

#[cfg(feature = "test-support")]
#[derive(Default)]
pub(super) struct Memory {
    owned: bool,
}
#[cfg(feature = "test-support")]
impl Transport for Memory {
    fn acquire(&mut self, _recover: bool) -> Result<(), Error> {
        if self.owned {
            return Err(Error::ExistingMaster);
        }
        self.owned = true;
        Ok(())
    }
    fn release(&mut self) {
        self.owned = false;
    }
    fn publish(&mut self, _publication: &TuningPublication) {}
    fn clients(&self) -> Result<u32, Error> {
        if self.owned {
            Ok(0)
        } else {
            Err(Error::LibraryUnavailable)
        }
    }
}
