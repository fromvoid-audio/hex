//! Independent prepared-callback fixtures share one integration-test process.
//! The allocation counter measures each test thread independently. Keep fixtures
//! and measured scopes local to each test.

mod audio_event_placement_allocations;

mod callback_allocations;
mod callback_session_batch;
mod controller_keyboard_allocations;
mod delay_timing_fork_allocations;
mod eq8_audition_allocations;
mod eq8_oversampling_study;
mod fresh_publication_cancellation;
mod limiter_allocations;
mod main_chain_allocations;
mod manual_host_gain_allocations;

mod midi_once_allocations;
mod operator_filter_proof_allocations;
mod operator_fixed_allocations;
mod pitch_multiplier_allocations;
mod return_feed_allocations;
mod routed_midi_allocations;
mod session_audio_loop_allocations;
mod session_silence_effect_allocations;
mod shared_source_allocations;
mod signed_envelope_allocations;
mod spectrum_capture_allocations;
mod stream_miss_fade;
mod streamed_loop_seam;
mod streamed_publication_boundary;
mod touch_playback_allocations;
mod track_gain_allocations;
mod vector_global_allocations;
mod warp_preview_metadata_cost;
