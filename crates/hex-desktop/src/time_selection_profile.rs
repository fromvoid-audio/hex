//! Disposable profiler startup and bounded, untimed report publication.
use gpui::Application;
use std::{
    ffi::OsString,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{TryRecvError, sync_channel},
    },
    time::{Duration, Instant},
};

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let directory = arguments
        .next()
        .ok_or("profiling requires a fixture directory")?;
    if arguments.next().is_some() {
        return Err("profiling accepts exactly one fixture directory".into());
    }
    let directory = PathBuf::from(directory);
    if !directory.is_absolute() {
        return Err("profiling requires an absolute fixture directory".into());
    }
    let directory = std::fs::canonicalize(directory).map_err(|error| error.to_string())?;
    for name in ["1.hex", "100.hex", "shared.wav"] {
        if !directory.join(name).is_file() {
            return Err(format!("profiling fixture missing: {name}"));
        }
    }
    let settings = directory.join(format!("native-profile-{}", std::process::id()));
    std::fs::create_dir(&settings)
        .map_err(|error| format!("could not create isolated profiling settings: {error}"))?;
    let (sender, receiver) = sync_channel(1);
    let (published, completion) = sync_channel(1);
    let publishing = Arc::new(AtomicBool::new(false));
    let writer_publishing = publishing.clone();
    let _writer = std::thread::Builder::new()
        .name("time-selection-report".into())
        .spawn(move || {
            let outcome = receiver
                .recv()
                .unwrap_or_else(|_| Err("profiling closed before completion; failures=1".into()));
            writer_publishing.store(true, Ordering::Release);
            let success = publish(outcome, std::io::stdout().lock(), std::io::stderr().lock());
            let _ = published.try_send(success);
        })
        .map_err(|error| format!("could not start profiling report writer: {error}"))?;
    let completed = Arc::new(AtomicBool::new(false));
    let run_completed = completed.clone();
    let quit_completed = completed.clone();
    let application = Application::new().with_assets(hex_ui::assets::HexAssets);
    application.run(move |cx| {
        // macOS terminate: does not return from Application::run. Publication must
        // finish first; an external quit before that must never report exit zero.
        cx.on_app_quit(move |_| {
            if !quit_completed.load(Ordering::Acquire) {
                std::process::exit(1);
            }
            async {}
        })
        .detach();
        let acknowledged = completed.clone();
        cx.spawn(async move |cx| {
            let mut publication_started = None;
            loop {
                match completion.try_recv() {
                    Ok(true) => {
                        acknowledged.store(true, Ordering::Release);
                        let _ = cx.update(|cx| cx.quit());
                        break;
                    }
                    Ok(false) | Err(TryRecvError::Disconnected) => std::process::exit(1),
                    Err(TryRecvError::Empty) => {}
                }
                // Only bound publication itself, not the finite measurement run.
                if publishing.load(Ordering::Acquire) && publication_started.is_none() {
                    publication_started = Some(Instant::now());
                }
                if publication_started
                    .is_some_and(|started| started.elapsed() > Duration::from_secs(30))
                {
                    std::process::exit(1);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(25))
                    .await;
            }
        })
        .detach();
        if let Err(error) =
            hex_ui::open_time_selection_profile(cx, directory, settings, sender.clone())
        {
            let _ = sender.try_send(Err(error));
        }
    });
    if run_completed.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err("profiling application stopped before publication; failures=1".into())
    }
}

fn publish(
    outcome: Result<String, String>,
    mut stdout: impl Write,
    mut stderr: impl Write,
) -> bool {
    match outcome {
        Ok(csv) => match stdout
            .write_all(csv.as_bytes())
            .and_then(|()| stdout.flush())
        {
            Ok(()) => true,
            Err(error) => {
                let _ = writeln!(stderr, "profiling output failed; failures=1: {error}")
                    .and_then(|()| stderr.flush());
                false
            }
        },
        Err(error) => {
            let _ = writeln!(stderr, "{error}").and_then(|()| stderr.flush());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::publish;
    use std::io::{self, Write};

    #[derive(Default)]
    struct Output {
        bytes: Vec<u8>,
        flushed: bool,
        fail_flush: bool,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushed = true;
            if self.fail_flush {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed output"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn success_requires_complete_csv_and_flush_before_acknowledgement() {
        let mut stdout = Output::default();
        let mut stderr = Output::default();
        assert!(publish(
            Ok("header\nrow\n".into()),
            &mut stdout,
            &mut stderr
        ));
        assert_eq!(stdout.bytes, b"header\nrow\n");
        assert!(stdout.flushed);
        assert!(stderr.bytes.is_empty());

        stdout.fail_flush = true;
        assert!(!publish(Ok("csv".into()), &mut stdout, &mut stderr));
        assert!(stderr.flushed);
        assert!(String::from_utf8_lossy(&stderr.bytes).contains("failures=1"));
    }

    #[test]
    fn failed_run_preserves_partial_report_and_never_acknowledges_success() {
        let mut stdout = Output::default();
        let mut stderr = Output::default();
        let report = "failures=1; completed_samples=2; timeout\nheader\nrow\n";
        assert!(!publish(Err(report.into()), &mut stdout, &mut stderr));
        assert!(stdout.bytes.is_empty());
        assert_eq!(stderr.bytes, format!("{report}\n").as_bytes());
        assert!(stderr.flushed);
    }
}
