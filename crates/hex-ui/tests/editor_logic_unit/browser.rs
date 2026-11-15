#[path = "browser_tree.rs"]
mod tree;

#[path = "browser_samples.rs"]
mod samples;

#[path = "../../src/editor/browser/scan.rs"]
mod scan;

#[allow(dead_code)]
#[path = "../../src/editor/browser/geometry.rs"]
mod geometry;

#[path = "../../src/editor/browser/audition_geometry.rs"]
mod audition_geometry;
#[path = "../../src/editor/browser/random_rank.rs"]
mod random_rank;
#[allow(dead_code)] // Runtime uses generation/row identity fields absent from this unit harness.
#[path = "../../src/editor/browser/scrollbar_geometry.rs"]
mod scrollbar_geometry;
