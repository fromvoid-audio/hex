//! Pause retains the original held worker; Release must survive until resume.
use super::*;
use crate::plugin_worker::{
    NativeParameterHold as Hold, NativeWorkerParameterProgress as Progress,
};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn paused_ordinary_hold_releases_before_resumed_authored_pcm() {
    const CHILD: &str = "HEX_ORDINARY_PAUSED_HOLD_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        return;
    }
    let _serial = serial();
    const QUANTUM: usize = 256;
    let mut f = fixture_with_authored("processing", 2, QUANTUM as u16, &[7]);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 64,
            enabled: true,
        })
        .unwrap();
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let render = |f: &mut OrdinaryFixture| {
        let output = physical_block::<QUANTUM>(f);
        let mut committed = None;
        f.data.owner_mut().unwrap().run(|callback, _| {
            committed = callback.native_sources.test_committed_frames(f.binding);
        });
        replacement::wait_for_blocks(&root, f.binding, committed.unwrap() / QUANTUM as u64);
        output
    };
    f.control.command(PlaybackCommand::Play).unwrap();
    render(&mut f);
    let source = f.control.status.native_effect_source().unwrap();
    let send = |f: &mut OrdinaryFixture, command| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .native_source_parameter_hold_on_control(source, f.binding, command)
            {
                Ok(token) => break token,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                Err(error) => panic!("paused hold admission: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    };
    let receive = |f: &mut OrdinaryFixture, token| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .poll_native_source_parameter_change_on_control(f.binding, token)
            {
                Ok(Progress::Pending) | Err("native preparation is busy") => {}
                Ok(terminal) => break terminal,
                Err(error) => panic!("paused hold terminal: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    };
    let hold = send(
        &mut f,
        Hold::Begin {
            id: 7,
            normalized: 0.75,
        },
    );
    render(&mut f);
    assert!(matches!(receive(&mut f, &hold), Progress::Processed(_)));
    render(&mut f);
    assert_eq!(render(&mut f), [[0.75, -1.5]; QUANTUM]);

    f.control.command(PlaybackCommand::Pause).unwrap();
    render(&mut f);
    assert!(!f.control.status.playing.load(Acquire));
    let release = send(&mut f, Hold::Release { hold: &hold });
    assert_eq!(
        f.control
            .poll_native_source_parameter_change_on_control(f.binding, &release)
            .unwrap(),
        Progress::Pending
    );
    f.control.command(PlaybackCommand::PlayFrom(0)).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while !f.control.status.playing.load(Acquire) {
        f.control.poll_resident_publication();
        render(&mut f);
        assert!(Instant::now() < until);
    }
    for _ in 0..3 {
        render(&mut f);
    }
    assert!(matches!(
        receive(&mut f, &release),
        Progress::Released { .. }
    ));
    assert_eq!(render(&mut f), [[0.25, -0.5]; QUANTUM]);
    assert_eq!(f.control.status.native_effect_source(), Some(source));
    dispose(f);
}
