//! Isolated real callback and limiter stage-clock allocation regression.
#![cfg(feature = "allocation-probe")]

#[path = "allocation_probes/callback_allocations/probe.rs"]
mod probe;
#[path = "allocation_probes/callback_allocations/stage_clocks.rs"]
mod stage_clocks;

use hex_audio::prepared;
use hex_engine::{
    Audio, Frame, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use probe::{compare_output, measured_block, measured_shutdown};
use std::{collections::BTreeMap, error::Error, sync::Arc};

const RATE: u32 = 48_000;
