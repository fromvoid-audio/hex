# Pulse task #2470: Add extensible shared device layout and section primitives

Operation: c7ad0967bcf64b8b5f5ee2b3fec7aa2a
Assignee: PORTALSURFER (Pulse #1, GitHub ID 41680373)

Extend hex-widgets rather than creating another GUI library. Extract Operator/Vector's accepted surround/header/rounded-section hierarchy into reusable frame, body, section, rail/grouped-row and display compositions. One canonical compact header height, header padding/gaps, radii, insets and gutter rules must serve every device; remove device-selected header-density variants from production dispatch. Normal idle header and outer surround share a surface without a separating dark rule.

Flexibility is required: shared row/column/grid/split composition and semantic section/display variants must cover short/tall sections, graphs, tabbed inspectors and wrapped parameter layouts. Keep useful per-device widths and parameter arrangement. Avoid one monolithic template, device-named appearance variants or type-ID-based style selection. If new functionality requires a visual capability, add a generic feature to the library first, then consume it.

Panels own parameter semantics and bindings; hex-widgets owns rendering/layout states, geometry and shared interaction primitives. Keep GPUI/std boundary and injected product colors. Preserve Operator focus, tab slots, grouping and gesture behavior.

Acceptance: Operator can retain accepted composition using these primitives; Sine and effects consume the same shell metrics. Meaningful rendered geometry/containment tests at normal and constrained sizes prove stable header/body bounds and rounded clipping. Document the library extension rule/API in hex-widgets README and device-controls docs. Related foundations #44/#2242; source frame.rs, style.rs and operator_panel.rs at audited source.
