//! Section clip-layout preview cost on a saved project, read-only.
//! Usage: `cargo run --release -p hex-session --example profile_section_layout -- <project.hex>`
//! Measures `Project::preview_section_layout` plus an upper bound for the UI's
//! display-only MIDI summaries (every clip rebuilt); excludes decode, UI paint,
//! worker preparation and audio publication. The file is never written.
use hex_project::{Command, Project};
use std::{hint::black_box, time::Instant};

const RUNS: u32 = 20;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: profile_section_layout <project.hex>")?;
    let project = hex_storage::decode(&std::fs::read(path)?)?;
    let sections = project.arrangement_sections();
    let clips: usize = project
        .tracks()
        .iter()
        .map(|t| t.clips().len() + t.midi_clips().len())
        .sum();
    println!(
        "tracks {} clips {} sections {}",
        project.tracks().len(),
        clips,
        sections.len()
    );
    let (Some(first), Some(last)) = (sections.first(), sections.last()) else {
        return Err("project has no arrangement sections".into());
    };
    let (first, last) = (first.id(), last.id());
    for (label, command) in [
        (
            "reorder first after last",
            Command::ReorderSectionAfter {
                section: first,
                after: last,
            },
        ),
        (
            "duplicate first",
            Command::DuplicateArrangementSection(first),
        ),
        (
            "duplicate last at first",
            Command::DuplicateArrangementSectionAt {
                section: last,
                target: first,
                after: false,
            },
        ),
        ("delete first", Command::DeleteArrangementSection(first)),
    ] {
        report(label, &project, &command)?;
    }
    let clone = time(|| black_box(project.clone()));
    println!("project clone alone: {clone:?} per run");
    Ok(())
}

fn report(
    label: &str,
    project: &Project,
    command: &Command,
) -> Result<(), Box<dyn std::error::Error>> {
    let preview = project.preview_section_layout(command)?;
    let layout = time(|| black_box(project.preview_section_layout(command)));
    let midi = time(|| {
        for clip in preview.tracks().iter().flat_map(|t| t.midi_clips()) {
            black_box(hex_engine::midi_clip_preview(&preview, clip).ok());
        }
    });
    println!("{label}: layout {layout:?}, all midi summaries {midi:?} (mean of {RUNS})");
    Ok(())
}

fn time<T>(mut run: impl FnMut() -> T) -> std::time::Duration {
    let start = Instant::now();
    for _ in 0..RUNS {
        black_box(run());
    }
    start.elapsed() / RUNS
}
