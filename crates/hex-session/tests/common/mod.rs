//! Shared helpers for session integration tests.

use hex_session::service::{Event, Service};
use std::{
    thread,
    time::{Duration, Instant},
};

pub fn wait(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("worker event missing: {error}"),
        }
    }
}
