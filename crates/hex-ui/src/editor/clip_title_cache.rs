//! Bounded LRU cache of shaped Arrangement clip titles.
//!
//! Shaping clip names through CoreText on every pan or zoom frame stalls the
//! main thread once dozens of clips show readable headers. Caching `ShapedLine`
//! by `(lowercase_text, font_size, color)` and clipping with a `ContentMask`
//! eliminates repeated text shaping across pan and zoom while keeping crisp
//! bundled Ioskeley Mono typography.
use crate::theme;
use gpui::{
    App, Bounds, ContentMask, Hsla, Pixels, ShapedLine, SharedString, TextRun, Window, point, px,
    rgb,
};
use std::collections::HashMap;

const MAX_CACHED_TITLES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TitleKey {
    text: SharedString,
    font_size_bits: u32,
    color: u32,
}

struct Entry {
    line: ShapedLine,
    last_used: u64,
}

#[derive(Clone, Copy)]
pub(super) struct TitlePaintStyle {
    pub(super) font_size: Pixels,
    pub(super) line_height: Pixels,
    pub(super) color: u32,
}

#[derive(Default)]
pub(super) struct ClipTitleCache {
    entries: HashMap<TitleKey, Entry>,
    clock: u64,
}

impl ClipTitleCache {
    pub(super) fn get_or_shape(
        &mut self,
        raw_name: &str,
        font_size: Pixels,
        color: u32,
        window: &mut Window,
    ) -> &ShapedLine {
        let text = theme::text(raw_name);
        let key = TitleKey {
            text,
            font_size_bits: f32::from(font_size).to_bits(),
            color,
        };
        self.clock = self.clock.wrapping_add(1);
        let now = self.clock;
        if self.entries.len() >= MAX_CACHED_TITLES && !self.entries.contains_key(&key) {
            self.evict_oldest();
        }
        let entry = self.entries.entry(key).or_insert_with_key(|key| {
            let font = window.text_style().font();
            let run = TextRun {
                len: key.text.len(),
                font,
                color: Hsla::from(rgb(color)),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window
                .text_system()
                .shape_line(key.text.clone(), font_size, &[run], None);
            Entry {
                line,
                last_used: now,
            }
        });
        entry.last_used = now;
        &entry.line
    }

    pub(super) fn paint_clipped(
        &mut self,
        raw_name: &str,
        bounds: Bounds<Pixels>,
        style: TitlePaintStyle,
        window: &mut Window,
        cx: &mut App,
    ) {
        if raw_name.is_empty() || bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return;
        }
        let line = self.get_or_shape(raw_name, style.font_size, style.color, window);
        let origin = point(bounds.left(), bounds.top());
        let clipped = bounds.intersect(&window.content_mask().bounds);
        if clipped.is_empty() {
            return;
        }
        window.with_content_mask(Some(ContentMask { bounds: clipped }), |window| {
            let _ = line.paint(origin, style.line_height, window, cx);
        });
    }

    fn evict_oldest(&mut self) {
        let target = MAX_CACHED_TITLES / 2;
        if self.entries.len() <= target {
            return;
        }
        let mut ages: Vec<u64> = self.entries.values().map(|e| e.last_used).collect();
        ages.sort_unstable();
        let cutoff = ages[ages.len() - target];
        self.entries.retain(|_, entry| entry.last_used >= cutoff);
    }
}
