//! # Purpose
//! Compose canonical headless project, import, recovery and WAV-export commands.
//!
//! # Dependencies
//! Uses hex-project for edits, hex-engine for preparation, and hex-media, hex-storage and
//! hex-export for file workflows.
//!
//! # Does not own
//! Does not own a second project model, desktop UI, native device lifecycle or a separate export
//! codec.
use hex_engine::{Audio, Frame, Tempo, compile};
mod import;
mod projects;
mod recovery;
mod wav;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;
use std::{error::Error, path::Path};

const USAGE: &str = "\
usage: hex-cli <command> [args...]

commands:
  demo <new-output.wav>
  edit-demo <new-output.wav>
  render-wav <input.wav> <new-output.wav>
  import-project <input.wav> <new-project.hex>
  render-project <project.hex> <new-output.wav>
  export-project <project.hex> <destination> <start-frame> <end-frame> [--tracks=ids|--returns=ids|--stems=track-ids] [--format=pcm16|pcm24|float32] [--overwrite]
  recover-project <target.hex> <new-project.hex>
  --help | -h | help";

fn is_help_request(args: &[std::ffi::OsString]) -> bool {
    args.len() == 1
        && args[0]
            .to_str()
            .is_some_and(|arg| matches!(arg, "--help" | "-h" | "help"))
}

fn unknown_command_usage(command: Option<&std::ffi::OsStr>) -> String {
    match command {
        Some(command) => format!("unknown command: {}\n{USAGE}", command.to_string_lossy()),
        None => USAGE.to_owned(),
    }
}

fn main() {
    if let Err(error) = run() {
        // Error text carries case-sensitive paths; never lowercase it.
        eprintln!("{}", hex_storage::text::plain(&error.to_string()));
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if is_help_request(&args) {
        println!("{USAGE}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "recover-project") {
        if args.len() != 3 {
            return Err("usage: hex-cli recover-project <target.hex> <new-project.hex>".into());
        }
        return recovery::recover_project(Path::new(&args[1]), Path::new(&args[2]));
    }
    if args.first().is_some_and(|a| a == "import-project") {
        if args.len() != 3 {
            return Err("usage: hex-cli import-project <input.wav> <new-project.hex>".into());
        }
        return projects::import_project(Path::new(&args[1]), Path::new(&args[2]));
    }
    if args.first().is_some_and(|a| a == "export-project") {
        return projects::export_project(&args);
    }
    if args.first().is_some_and(|a| a == "render-project") {
        if args.len() != 3 {
            return Err("usage: hex-cli render-project <project.hex> <new-output.wav>".into());
        }
        return projects::render_project(Path::new(&args[1]), Path::new(&args[2]));
    }
    if args.first().is_some_and(|a| a == "render-wav") {
        if args.len() != 3 {
            return Err("usage: hex-cli render-wav <input.wav> <new-output.wav>".into());
        }
        return import::render(Path::new(&args[1]), Path::new(&args[2]));
    }
    if args
        .first()
        .is_some_and(|a| a == "demo" || a == "edit-demo")
    {
        if args.len() != 2 {
            return Err("usage: hex-cli <demo|edit-demo> <new-output.wav>".into());
        }
    } else {
        return Err(unknown_command_usage(args.first().map(|a| a.as_os_str())).into());
    }
    let rate = 48_000;
    let tempo = Tempo::new(120.0)?;
    let beat = tempo.frames_at(1.0, rate)?;
    let source: Vec<Frame> = (0..beat)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            let envelope = (1.0 - i as f64 / beat as f64).powi(2);
            let s = (std::f64::consts::TAU * 220.0 * t).sin() * envelope * 0.3;
            [s as f32; 2]
        })
        .collect();
    let audio = Audio::new(rate, source)?;
    let mut editor = Editor::new(Project::new(rate)?, 32)?;
    let track = match editor.execute(Command::AddTrack {
        name: "first light".into(),
    })? {
        Created::Track(id) => id,
        _ => return Err("track creation did not return its id".into()),
    };
    let asset = match editor.execute(Command::AddAsset {
        path: "generated/first-light".into(),
        sample_rate: rate,
        frames: beat,
    })? {
        Created::Asset(id) => id,
        _ => return Err("asset creation did not return its id".into()),
    };
    let clip = match editor.execute(Command::PlaceClip {
        track,
        asset,
        start: beat,
        duration: beat * 7,
        source_offset: 0,
        source_length: beat,
        looping: true,
    })? {
        Created::Clip(id) => id,
        _ => return Err("clip creation did not return its id".into()),
    };
    if args[0] == "edit-demo" {
        editor.execute(Command::TrimClip {
            clip,
            start: beat * 2,
            duration: beat * 6,
            source_offset: 0,
            source_length: beat / 2,
        })?;
        editor.execute(Command::Gain { track, gain: 0.5 })?;
        if !editor.try_undo()? || !editor.try_redo()? {
            return Err("edit history could not restore the gain change".into());
        }
    }
    let media = BTreeMap::from([(asset, audio)]);
    let project = compile::project(editor.project(), &media)?;
    wav::render(Path::new(&args[1]), &project, beat * 8)?;
    println!("rendered {} stereo frames at {} hz", beat * 8, rate);
    Ok(())
}
