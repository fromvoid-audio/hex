
#[test]
fn rms_pcm_matches_analytic_dc_burst_energy_then_original_attack_release_and_makeup_mix() {
    for rate in [8000_u32,22050,44100,48000,96000,192000] {
        for (attack,release) in [(0.1_f32,5.0_f32),(100.0,1000.0)] {
            let mut state=mode(&COMPRESSOR_DEFINITION.default_state().unwrap(),1);
            for (id,value) in [(parameter::THRESHOLD,-24.0),(parameter::KNEE,0.0),(parameter::MAKEUP,6.0),(parameter::DRY_WET,0.375),(parameter::ATTACK,attack),(parameter::RELEASE,release)] {state=patch(&state,id,value);}
            let mut actual=runtime(&state,rate);
            let a=(-100.0/f64::from(rate)).exp();let attack_a=(-1000.0/(f64::from(attack)*f64::from(rate))).exp();let release_a=(-1000.0/(f64::from(release)*f64::from(rate))).exp();let gain_a=(-200.0/f64::from(rate)).exp();
            let burst=rate as usize/100;let mut env=0.0;let mut gain_db=6.0;
            for n in 1..=2*burst {
                let e=if n<=burst {0.25*(1.0-a.powi(n as i32))}else{0.25*(1.0-a.powi(burst as i32))*a.powi((n-burst) as i32)};
                let detector=e.sqrt();let coefficient=if detector>env {attack_a}else{release_a};env=coefficient*env+(1.0-coefficient)*detector;
                let level=if env>0.0 {20.0*env.log10()}else{-120.0};let target=6.0-0.75*(level+24.0).max(0.0);gain_db=gain_a*gain_db+(1.0-gain_a)*target;
                let input=if n<=burst {[0.5,-0.25]}else{[0.0,0.0]};let mut frame=[input];actual.process_block(&mut frame,&hex_lattice::EMPTY_DEVICE_RESOURCES);
                let h=history(actual.as_ref());assert!((h.envelope-env).abs()<2e-12);assert!((h.gain_db-gain_db).abs()<2e-9);
                for c in 0..2 {let x=f64::from(input[c]);let expected=x*0.625+x*10.0_f64.powf(gain_db/20.0)*0.375;assert!((f64::from(frame[0][c])-expected).abs()<1e-7,"actual PCM rate{rate} frame{n}");}
            }
        }
    }
}

#[test]
fn rms_quadrature_channels_have_independently_averaged_energy_and_choice_preserves_host_metadata() {
    let base=COMPRESSOR_DEFINITION.default_state().unwrap().with_input_gain_millidb(-3210).unwrap().with_output_gain_millidb(1234).unwrap();let state=mode(&base,1);assert_eq!(state.input_gain_millidb(),-3210);assert_eq!(state.output_gain_millidb(),1234);
    let mut pan=runtime(&patch(&state,parameter::DRY_WET,0.0),48000);
    let cycle:Vec<_>=(0..48).map(|i|{let phase=std::f64::consts::TAU*i as f64/48.0;[phase.sin() as f32,phase.cos() as f32]}).collect();
    pan.process_block(&mut (0..24000).map(|i|cycle[i%48]).collect::<Vec<_>>(),&hex_lattice::EMPTY_DEVICE_RESOURCES);
    let a=(-100.0_f64/48000.0).exp();
    for phase in 0..48 {
        pan.process_block(&mut [cycle[phase]],&hex_lattice::EMPTY_DEVICE_RESOURCES);let e=history(pan.as_ref()).energy;
        for c in 0..2 {let expected=(1.0-a)/(1.0-a.powi(48))*(0..48).map(|k|a.powi(k as i32)*f64::from(cycle[(phase+48-k)%48][c]).powi(2)).sum::<f64>();assert!((e[c]-expected).abs()<1e-13);}
        assert!(e[0].max(e[1])<0.51,"not instantaneous stereo-max power");
    }
}
