#[cfg(test)]
mod tests {
    use hex_devices::compressor::{COMPRESSOR_DEFINITION,parameter};
    use hex_lattice::{NativeDeviceDefinition,NativeProcessorRuntime,DevicePreparationContext,EMPTY_DEVICE_RESOURCES,TuningTable};
    use hex_lattice_contract::{NativeDeviceMetadata,ParameterValue};
    fn runtime(rate:u32,mix:f32,mode:u32)->Box<dyn NativeProcessorRuntime> {
        let state=COMPRESSOR_DEFINITION.default_state().unwrap();
        let state=COMPRESSOR_DEFINITION.patch_parameter(&state,parameter::DRY_WET,ParameterValue::Float(mix)).unwrap();
        let state=COMPRESSOR_DEFINITION.patch_parameter(&state,parameter::DETECTOR,ParameterValue::Choice(mode)).unwrap();
        COMPRESSOR_DEFINITION.prepare_processor(&state,&DevicePreparationContext {sample_rate:rate,tuning:&TuningTable::default(),resources:&EMPTY_DEVICE_RESOURCES}).unwrap().unwrap().create_runtime()
    }
    #[test]
    fn rms_and_mix_processing_bypass_reset_history_transfer_allocate_and_free_nothing() {
        // Sensitivity check distinguishes zero allocations from net-zero churn.
        let calibration=allocation_counter::measure(|| {
            let allocation=vec![std::hint::black_box(1_u8);4096];
            std::hint::black_box(&allocation);
        });
        assert!(calibration.count_total>0 && calibration.bytes_total>=4096);
        assert_eq!((calibration.count_current,calibration.bytes_current),(0,0));
        for rate in [8000,22050,44100,48000,96000,192000] {
            for mode in 0..2 {
            for mix in [0.0_f32,0.375,1.0] {
                let mut prior=runtime(rate,1.0-mix,1-mode);
                let mut active=runtime(rate,mix,mode);
                let mut scratch=runtime(rate,mix,mode);
                let mut frames=[[0.25,-0.07];257];
                prior.process_block(&mut frames,&EMPTY_DEVICE_RESOURCES);
                let mut silence=[[0.0;2];257];
                let measure=allocation_counter::measure(|| {
                    active.copy_history_from(prior.as_ref());
                    for _ in 0..8 {
                        scratch.copy_history_from(active.as_ref());
                        active.process_block(&mut frames,&EMPTY_DEVICE_RESOURCES);
                        scratch.process_bypassed_block(&mut frames,&EMPTY_DEVICE_RESOURCES);
                    }
                    // Both histories cross the real energy floor during this
                    // measured silence. Stack scratch, preparation outside probe.
                    for _ in 0..rate.div_ceil(257) {
                        active.process_block(&mut silence,&EMPTY_DEVICE_RESOURCES);
                        scratch.process_bypassed_block(&mut silence,&EMPTY_DEVICE_RESOURCES);
                    }
                    active.reset();
                    scratch.reset();
                    std::hint::black_box(&frames);
                });
                println!("ENERGY_FLOOR_ALLOC rate={rate} mix={mix} mode={mode} {measure:?}");
                assert_eq!((measure.count_total,measure.bytes_total,measure.count_current,measure.bytes_current),(0,0,0,0),"rate={rate} mix={mix} mode={mode}");
            }
            }
        }
    }
}
