#[cfg(test)]
mod tests {
    use hex_devices::auto_pan::{AUTO_PAN_DEFINITION,parameter};
    use hex_lattice::{NativeDeviceDefinition,NativeProcessorRuntime,DevicePreparationContext,EMPTY_DEVICE_RESOURCES,TuningTable};
    use hex_lattice_contract::{NativeDeviceMetadata,ParameterValue};
    fn runtime(rate:u32,shape:u32)->Box<dyn NativeProcessorRuntime> {
        let state=AUTO_PAN_DEFINITION.default_state().unwrap();
        let state=AUTO_PAN_DEFINITION.patch_parameter(&state,parameter::WAVEFORM,ParameterValue::Choice(shape)).unwrap();
        AUTO_PAN_DEFINITION.prepare_processor(&state,&DevicePreparationContext {sample_rate:rate,tuning:&TuningTable::default(),resources:&EMPTY_DEVICE_RESOURCES}).unwrap().unwrap().create_runtime()
    }
    #[test]
    fn mix_processing_bypass_reset_history_transfer_allocate_and_free_nothing() {
        // Sensitivity check distinguishes zero allocations from net-zero churn.
        let calibration=allocation_counter::measure(|| {
            let allocation=vec![std::hint::black_box(1_u8);4096];
            std::hint::black_box(&allocation);
        });
        assert!(calibration.count_total>0 && calibration.bytes_total>=4096);
        assert_eq!((calibration.count_current,calibration.bytes_current),(0,0));
        for rate in [8000,22050,44100,48000,96000,192000] {
            for shape in 0..5 {
                let mut prior=runtime(rate,(shape+1)%5);
                let mut active=runtime(rate,shape);
                let mut scratch=runtime(rate,shape);
                let mut frames=[[0.25,-0.07];257];
                prior.process_block(&mut frames,&EMPTY_DEVICE_RESOURCES);
                let measure=allocation_counter::measure(|| {
                    active.copy_history_from(prior.as_ref());
                    for _ in 0..8 {
                        scratch.copy_history_from(active.as_ref());
                        active.process_block(&mut frames,&EMPTY_DEVICE_RESOURCES);
                        scratch.process_bypassed_block(&mut frames,&EMPTY_DEVICE_RESOURCES);
                    }
                    active.reset();
                    scratch.reset();
                    std::hint::black_box(&frames);
                });
                assert_eq!((measure.count_total,measure.bytes_total,measure.count_current,measure.bytes_current),(0,0,0,0),"rate={rate} shape={shape}");
            }
        }
    }
}
