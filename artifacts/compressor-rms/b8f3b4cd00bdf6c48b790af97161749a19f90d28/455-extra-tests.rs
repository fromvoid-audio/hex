
fn mode(state: &NativeDeviceState, value: u32) -> NativeDeviceState {
    registry().patch_parameter(state, parameter::DETECTOR, ParameterValue::Choice(value)).unwrap()
}
fn history(value: &dyn NativeProcessorRuntime) -> &CompressorRuntime {
    value.as_any().downcast_ref::<CompressorRuntime>().unwrap()
}

#[test]
fn rms_energy_matches_dc_impulse_and_burst_closed_forms_at_six_rates() {
    let state = mode(&patch(&COMPRESSOR_DEFINITION.default_state().unwrap(), parameter::DRY_WET, 0.0), 1);
    for rate in [8000_u32,22050,44100,48000,96000,192000] {
        let a = (-100.0 / f64::from(rate)).exp();
        let n = rate as usize / 100;
        let mut dc = runtime(&state,rate);
        dc.process_block(&mut vec![[0.5,-0.25];n],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        let expected = 0.25 * (1.0-a.powi(n as i32));
        assert!((history(dc.as_ref()).energy[0]-expected).abs()<2e-14);
        assert!((history(dc.as_ref()).energy[1]-expected/4.0).abs()<2e-14);
        let mut impulse = runtime(&state,rate);
        impulse.process_block(&mut [[0.5,-0.25]],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        impulse.process_block(&mut vec![[0.0;2];n],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!((history(impulse.as_ref()).energy[0]-(1.0-a)*0.25*a.powi(n as i32)).abs()<2e-14);
        dc.process_block(&mut vec![[0.0;2];n],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!((history(dc.as_ref()).energy[0]-expected*a.powi(n as i32)).abs()<2e-14);
        assert_eq!(history(dc.as_ref()).detector_mix,1.0);
    }
}

#[test]
fn rms_periodic_sine_uses_mean_square_not_mean_absolute_and_preserves_phase_ripple() {
    let state = mode(&patch(&COMPRESSOR_DEFINITION.default_state().unwrap(), parameter::DRY_WET,0.0),1);
    let cycle:Vec<f32>=(0..48).map(|i|(0.5*(std::f64::consts::TAU*i as f64/48.0).sin()) as f32).collect();
    for rate in [8000_u32,22050,44100,48000,96000,192000] {
        let a=(-100.0/f64::from(rate)).exp();
        let mut pan=runtime(&state,rate);
        let count=(rate as usize/2).div_ceil(48)*48;
        let mut warm:Vec<_>=(0..count).map(|i|[cycle[i%48],-cycle[i%48]]).collect();
        pan.process_block(&mut warm,&hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut sum=0.0;
        for phase in 0..48 {
            pan.process_block(&mut [[cycle[phase],-cycle[phase]]],&hex_lattice::EMPTY_DEVICE_RESOURCES);
            // Closed periodic convolution; independent of the streaming implementation.
            let expected=(1.0-a)/(1.0-a.powi(48))*(0..48).map(|k|a.powi(k as i32)*f64::from(cycle[(phase+48-k)%48]).powi(2)).sum::<f64>();
            let e=history(pan.as_ref()).energy;
            assert!((e[0]-expected).abs()<2e-13,"rate{rate} phase{phase}");
            assert_eq!(e[0],e[1]);sum+=e[0];
        }
        assert!((sum/48.0-0.125).abs()<1e-8,"sine mean energy, not squared mean absolute");
    }
}

#[test]
fn rms_links_max_after_channel_averaging_and_not_instantaneous_max() {
    let state=mode(&patch(&COMPRESSOR_DEFINITION.default_state().unwrap(),parameter::DRY_WET,0.0),1);
    let mut alternating=runtime(&state,48000);
    let mut input:Vec<_>=(0..24000).map(|i|if i%2==0 {[1.0,0.0]}else{[0.0,-1.0]}).collect();
    alternating.process_block(&mut input,&hex_lattice::EMPTY_DEVICE_RESOURCES);
    let e=history(alternating.as_ref()).energy;
    let a=(-100.0_f64/48000.0).exp();
    assert!((e[0]-a/(1.0+a)).abs()<1e-13);
    assert!((e[1]-1.0/(1.0+a)).abs()<1e-13);
    assert!(e[0].max(e[1])<0.501,"max before averaging would incorrectly give1");
    let mut left=runtime(&state,48000);let mut dual=runtime(&state,48000);
    left.process_block(&mut [[0.8,0.0];1000],&hex_lattice::EMPTY_DEVICE_RESOURCES);
    dual.process_block(&mut [[-0.8,0.8];1000],&hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(history(left.as_ref()).envelope,history(dual.as_ref()).envelope);
    assert_eq!(history(left.as_ref()).gain_db,history(dual.as_ref()).gain_db);
}

#[test]
fn rms_mode_history_ramps_survive_partitions_bypass_rapid_retarget_and_reset() {
    for rate in [8000_u32,22050,44100,48000,96000,192000] {
        let base=patch(&COMPRESSOR_DEFINITION.default_state().unwrap(),parameter::DRY_WET,0.0);
        let changed=mode(&patch(&base,parameter::DRY_WET,0.75),1);
        let mut committed=runtime(&base,rate);
        committed.process_block(&mut [[0.7,-0.23];1021],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        let count=rate.div_ceil(200) as usize;
        let mut whole=runtime(&changed,rate);whole.copy_history_from(committed.as_ref());
        let mut split=runtime(&changed,rate);split.copy_history_from(committed.as_ref());
        let energy=history(committed.as_ref()).energy;
        assert_eq!(history(whole.as_ref()).energy,energy,"warm energy survives Peak→RMS");
        let mut a=vec![[0.8,-0.27];count+13];let mut b=a.clone();
        whole.process_block(&mut a,&hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut start=0;
        for len in [1,7,31,2,503].into_iter().cycle() {
            if start==b.len(){break}let end=(start+len).min(b.len());
            let mut twin=runtime(&changed,rate);twin.copy_history_from(split.as_ref());
            twin.process_block(&mut b[start..end],&hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert!((history(twin.as_ref()).detector_mix-((end as f64/count as f64).min(1.0))).abs()<3e-14);
            split=twin;start=end;
        }
        assert_eq!(a,b);assert_eq!(history(whole.as_ref()).energy,history(split.as_ref()).energy);
        assert_eq!(history(whole.as_ref()).detector_remaining,0);
        assert_eq!(history(committed.as_ref()).energy,energy,"scratch did not mutate committed");
        let mut bypass=runtime(&changed,rate);bypass.copy_history_from(committed.as_ref());
        let mut dry=vec![[0.8,-0.27];count+13];bypass.process_bypassed_block(&mut dry,&hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(dry.iter().all(|s|*s==[0.8,-0.27]));
        let mut next=[[0.4,-0.2];23];let mut other=next;
        bypass.process_block(&mut next,&hex_lattice::EMPTY_DEVICE_RESOURCES);whole.process_block(&mut other,&hex_lattice::EMPTY_DEVICE_RESOURCES);assert_eq!(next,other);
        let mut partial=runtime(&changed,rate);partial.copy_history_from(committed.as_ref());
        partial.process_block(&mut [[0.8,-0.2]],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        let previous=history(partial.as_ref());let fraction=previous.detector_mix;
        let mut reverse=runtime(&base,rate);reverse.copy_history_from(partial.as_ref());
        assert_eq!(history(reverse.as_ref()).detector_mix,fraction);assert_eq!(history(reverse.as_ref()).detector_remaining,count as u32);
        reverse.process_block(&mut vec![[0.8,-0.2];count],&hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(history(reverse.as_ref()).detector_mix,0.0);
        split.reset();assert_eq!(history(split.as_ref()).energy,[0.0;2]);assert_eq!(history(split.as_ref()).detector_mix,1.0);
        let mut reset=[[0.8,-0.2];19];let mut fresh=reset;
        split.process_block(&mut reset,&hex_lattice::EMPTY_DEVICE_RESOURCES);runtime(&changed,rate).process_block(&mut fresh,&hex_lattice::EMPTY_DEVICE_RESOURCES);assert_eq!(reset,fresh);
    }
}

#[test]
fn rms_state_validation_invalid_frames_and_static_detector_preview_are_truthful() {
    let base=COMPRESSOR_DEFINITION.default_state().unwrap();assert_eq!(base.payload().len(),32);
    assert_eq!(COMPRESSOR_DEFINITION.read_parameter(&base,parameter::DETECTOR).unwrap(),ParameterValue::Choice(0));
    for value in [ParameterValue::Choice(2),ParameterValue::Choice(u32::MAX),ParameterValue::Float(f32::NAN),ParameterValue::Float(1.0),ParameterValue::Bool(true)] {
        assert!(registry().patch_parameter(&base,parameter::DETECTOR,value).is_err());
    }
    for (version,len) in [(1,24),(2,28)] {assert_eq!(registry().migrate_state(&NativeDeviceState::new(COMPRESSOR_TYPE_ID,version,vec![0;len]).unwrap()),Err(DeviceContractError::UnsupportedStateVersion));}
    let rms=mode(&base,1);assert_eq!(registry().migrate_state(&rms).unwrap(),rms);
    assert_eq!(transfer_curve(&base).unwrap(),transfer_curve(&rms).unwrap(),"same steady detector-level curve; no fictitious3dBshift");
    let mut corrupt=rms.payload().to_vec();corrupt[28..32].copy_from_slice(&2_u32.to_le_bytes());assert!(transfer_curve(&NativeDeviceState::new(COMPRESSOR_TYPE_ID,3,corrupt).unwrap()).is_err());
    let mut pan=runtime(&patch(&rms,parameter::DRY_WET,0.0),48000);
    pan.process_block(&mut [[0.8,-0.2];9],&hex_lattice::EMPTY_DEVICE_RESOURCES);
    let before=history(pan.as_ref());let saved=(before.energy,before.envelope,before.gain_db,before.detector_mix,before.detector_remaining);
    for frame in [[f32::NAN,1.0],[1.0,f32::INFINITY]] {pan.process_block(&mut [frame],&hex_lattice::EMPTY_DEVICE_RESOURCES);let h=history(pan.as_ref());assert_eq!((h.energy,h.envelope,h.gain_db,h.detector_mix,h.detector_remaining),saved);}
    for frame in [[f32::MAX,-f32::MAX],[f32::from_bits(1),0.0],[0.0,0.0]] {
        let mut sample=[frame];pan.process_block(&mut sample,&hex_lattice::EMPTY_DEVICE_RESOURCES);assert_eq!(sample[0],frame,"dry exact");let h=history(pan.as_ref());assert!(h.energy.iter().all(|v|v.is_finite())&&h.envelope.is_finite()&&h.gain_db.is_finite());
    }
}
