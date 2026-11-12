// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Deref, Range, Sub},
    slice,
    sync::Arc,
};

#[allow(non_camel_case_types, unused)]
pub(crate) type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

pub(crate) type DrawOrder = u32;

#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
}

impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let primitive = primitive.into();
        let clipped_bounds = primitive
            .painted_bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return;
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        self.append_primitive(primitive, order);
    }

    fn append_primitive(&mut self, mut primitive: Primitive, order: DrawOrder) {
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(shadow.clone());
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(quad.clone());
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(underline.clone());
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(sprite.clone());
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(sprite.clone());
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        if self.try_bulk_replay(range.clone(), prev_scene) {
            return;
        }
        self.replay_legacy(range, prev_scene);
    }

    // A cached subtree can retain its internal painter ordering. Reserving one
    // conservative union adds ordering constraints only between disjoint areas;
    // overlapping primitives and mixed-kind ties retain their original order.
    // Refused paths must not mutate any scene or bounds-tree state.
    fn try_bulk_replay(&mut self, range: Range<usize>, prev_scene: &Scene) -> bool {
        if !self.layer_stack.is_empty() {
            return false;
        }
        let Some(operations) = prev_scene.paint_operations.get(range.clone()) else {
            return false;
        };
        let mut depth = 0_usize;
        for operation in &prev_scene.paint_operations[..range.start] {
            match operation {
                PaintOperation::StartLayer(_) => {
                    let Some(next) = depth.checked_add(1) else {
                        return false;
                    };
                    depth = next;
                }
                PaintOperation::EndLayer => {
                    let Some(next) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = next;
                }
                PaintOperation::Primitive(_) => {}
            }
        }
        if depth != 0 {
            return false;
        }
        let mut union = None::<Bounds<ScaledPixels>>;
        let mut min_order = u32::MAX;
        let mut max_order = 0_u32;
        let mut visible = false;
        for operation in operations {
            let bounds = match operation {
                PaintOperation::Primitive(primitive) => {
                    if !finite_replay_bounds(primitive.bounds())
                        || !finite_replay_bounds(&primitive.content_mask().bounds)
                    {
                        return false;
                    }
                    let painted = primitive.painted_bounds();
                    if !finite_replay_bounds(&painted) {
                        return false;
                    }
                    let clipped = painted.intersect(&primitive.content_mask().bounds);
                    if clipped.is_empty() {
                        continue;
                    }
                    min_order = min_order.min(primitive.order());
                    max_order = max_order.max(primitive.order());
                    visible = true;
                    clipped
                }
                PaintOperation::StartLayer(bounds) => {
                    let Some(next) = depth.checked_add(1) else {
                        return false;
                    };
                    depth = next;
                    *bounds
                }
                PaintOperation::EndLayer => {
                    let Some(next) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = next;
                    continue;
                }
            };
            if !finite_replay_bounds(&bounds) {
                return false;
            }
            let combined = union.map_or(bounds, |union| union.union(&bounds));
            if !finite_replay_bounds(&combined) {
                return false;
            }
            union = Some(combined);
        }
        if depth != 0 || !visible {
            return false;
        }
        let Some(bounds) = union else {
            return false;
        };
        let span = max_order - min_order;
        let Some(base) = self.primitive_bounds.reserve(bounds, span) else {
            return false;
        };
        // All eligibility and overflow checks precede the reservation. Every
        // visible order lies in [min_order, max_order], so this addition fits.
        for operation in operations {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    if !primitive
                        .bounds()
                        .intersect(&primitive.content_mask().bounds)
                        .is_empty()
                    {
                        self.append_primitive(
                            primitive.clone(),
                            base + (primitive.order() - min_order),
                        );
                    }
                }
                PaintOperation::StartLayer(bounds) => self
                    .paint_operations
                    .push(PaintOperation::StartLayer(*bounds)),
                PaintOperation::EndLayer => self.paint_operations.push(PaintOperation::EndLayer),
            }
        }
        true
    }

    fn replay_legacy(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    pub fn finish(&mut self) {
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_by_key(|surface| surface.order);
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub(crate) fn batches(&self) -> impl Iterator<Item = PrimitiveBatch<'_>> {
        BatchIterator {
            shadows: &self.shadows,
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads: &self.quads,
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths: &self.paths,
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines: &self.underlines,
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites: &self.monochrome_sprites,
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            polychrome_sprites: &self.polychrome_sprites,
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces: &self.surfaces,
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
        }
    }
}

fn finite_replay_bounds(bounds: &Bounds<ScaledPixels>) -> bool {
    let values = [
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
        bounds.right().0,
        bounds.bottom().0,
    ];
    values.into_iter().all(f32::is_finite)
        && bounds.size.width.0 >= 0.0
        && bounds.size.height.0 >= 0.0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    PolychromeSprite,
    Surface,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
}

impl Primitive {
    fn order(&self) -> DrawOrder {
        match self {
            Primitive::Shadow(value) => value.order,
            Primitive::Quad(value) => value.order,
            Primitive::Path(value) => value.order,
            Primitive::Underline(value) => value.order,
            Primitive::MonochromeSprite(value) => value.order,
            Primitive::PolychromeSprite(value) => value.order,
            Primitive::Surface(value) => value.order,
        }
    }
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
        }
    }

    fn painted_bounds(&self) -> Bounds<ScaledPixels> {
        if let Self::MonochromeSprite(sprite) = self {
            return sprite.painted_bounds();
        }
        *self.bounds()
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows: &'a [Shadow],
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads: &'a [Quad],
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths: &'a [Path<ScaledPixels>],
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines: &'a [Underline],
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites: &'a [MonochromeSprite],
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    polychrome_sprites: &'a [PolychromeSprite],
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces: &'a [PaintSurface],
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(
                    &self.shadows[shadows_start..shadows_end],
                ))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(&self.quads[quads_start..quads_end]))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(&self.paths[paths_start..paths_end]))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(
                    &self.underlines[underlines_start..underlines_end],
                ))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sprites: &self.monochrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = self.polychrome_sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites: &self.polychrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(
                    &self.surfaces[surfaces_start..surfaces_end],
                ))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveBatch<'a> {
    Shadows(&'a [Shadow]),
    Quads(&'a [Quad]),
    Paths(&'a [Path<ScaledPixels>]),
    Underlines(&'a [Underline]),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [MonochromeSprite],
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [PolychromeSprite],
    },
    Surfaces(&'a [PaintSurface]),
}

#[derive(Default, Debug, Clone)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

impl MonochromeSprite {
    pub(crate) fn painted_bounds(&self) -> Bounds<ScaledPixels> {
        if self.transformation == TransformationMatrix::unit() {
            return self.bounds;
        }
        let transform = |point: Point<ScaledPixels>| {
            self.transformation
                .apply(point.map(|p| Pixels(p.0)))
                .map(|p| ScaledPixels(p.0))
        };
        let first = transform(self.bounds.origin);
        let mut min = first;
        let mut max = first;
        for corner in [
            self.bounds.top_right(),
            self.bounds.bottom_left(),
            self.bounds.bottom_right(),
        ] {
            let p = transform(corner);
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        Bounds::from_corners(min, max)
    }
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub grayscale: bool,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(target_os = "macos")]
    pub image_buffer: core_video::pixel_buffer::CVPixelBuffer,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathId(pub(crate) usize);

/// Mutable construction keeps a directly owned Vec; only finished scaled meshes
/// pay for sharing. Metadata remains in Path, separate from immutable vertex bytes.
#[derive(Clone)]
pub(crate) enum PathVertices<P: Clone + Debug + Default + PartialEq> {
    Owned(Vec<PathVertex<P>>),
    Shared(Arc<Vec<PathVertex<P>>>),
}
impl<P: Clone + Debug + Default + PartialEq> Deref for PathVertices<P> {
    type Target = [PathVertex<P>];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(vertices) => vertices.as_slice(),
            Self::Shared(vertices) => vertices.as_slice(),
        }
    }
}
impl<P: Clone + Debug + Default + PartialEq> Debug for PathVertices<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(formatter)
    }
}
impl<P: Clone + Debug + Default + PartialEq> PathVertices<P> {
    fn make_mut(&mut self) -> &mut Vec<PathVertex<P>> {
        match self {
            Self::Owned(vertices) => vertices,
            Self::Shared(vertices) => Arc::make_mut(vertices),
        }
    }
}

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub(crate) id: PathId,
    pub(crate) order: DrawOrder,
    pub(crate) bounds: Bounds<P>,
    pub(crate) content_mask: ContentMask<P>,
    // Scaled Scene clones share mesh bytes; mutable construction stays owned.
    pub(crate) vertices: PathVertices<P>,
    pub(crate) color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: PathVertices::Owned(Vec::new()),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: PathVertices::Shared(Arc::new(self.vertices.iter()
                .map(|vertex| vertex.scale(factor)).collect())),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        let vertices = self.vertices.make_mut();
        vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    pub(crate) fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub(crate) xy_position: Point<P>,
    pub(crate) st_position: Point<f32>,
    pub(crate) content_mask: ContentMask<P>,
}

impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn verify_bulk_replay() -> Result<(), String> {
    replay_verification::verify()
}

// Std-only fixtures run through Hex's existing root-locked test-support graph.
#[cfg(any(test, feature = "test-support"))]
mod replay_verification {
    use super::*;
    use crate::{AtlasTextureKind, TileId, rgba};

    fn require(value: bool, message: &str) -> Result<(), String> {
        if value {
            Ok(())
        } else {
            Err(message.to_owned())
        }
    }

    fn bounds(x: f32, y: f32, width: f32, height: f32, scale: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point((x * scale).into(), (y * scale).into()),
            size: Size {
                width: (width * scale).into(),
                height: (height * scale).into(),
            },
        }
    }

    fn primitive(
        kind: usize,
        id: u32,
        bounds: Bounds<ScaledPixels>,
        mask: Bounds<ScaledPixels>,
    ) -> Primitive {
        let color = Hsla {
            h: id as f32 / 256.0,
            s: 0.6,
            l: 0.4,
            a: 0.37,
        };
        let content_mask = ContentMask { bounds: mask };
        match kind % 6 {
            0 => Shadow {
                order: 0,
                blur_radius: 2.0.into(),
                bounds,
                corner_radii: Default::default(),
                content_mask,
                color,
            }
            .into(),
            1 => Quad {
                bounds,
                content_mask,
                background: rgba(0x3366cc80).into(),
                border_color: color,
                ..Default::default()
            }
            .into(),
            2 => Path {
                id: PathId(0),
                order: 0,
                bounds,
                content_mask: content_mask.clone(),
                vertices: PathVertices::Shared(Arc::new(vec![PathVertex {
                    xy_position: bounds.origin,
                    st_position: point(0.0, 0.0),
                    content_mask,
                }])),
                color: color.into(),
                start: bounds.origin,
                current: bounds.bottom_right(),
                contour_count: 1,
            }
            .into(),
            3 => Underline {
                order: 0,
                pad: id,
                bounds,
                content_mask,
                color,
                thickness: 1.0.into(),
                wavy: id % 2,
            }
            .into(),
            4 => MonochromeSprite {
                order: 0,
                pad: id,
                bounds,
                content_mask,
                color,
                tile: AtlasTile {
                    texture_id: AtlasTextureId {
                        index: id % 2,
                        kind: AtlasTextureKind::Monochrome,
                    },
                    tile_id: TileId(id % 3),
                    padding: 0,
                    bounds: Default::default(),
                },
                transformation: TransformationMatrix::unit(),
            }
            .into(),
            _ => PolychromeSprite {
                order: 0,
                pad: id,
                grayscale: id % 2 == 0,
                opacity: 0.37,
                bounds,
                content_mask,
                corner_radii: Default::default(),
                tile: AtlasTile {
                    texture_id: AtlasTextureId {
                        index: id % 2,
                        kind: AtlasTextureKind::Polychrome,
                    },
                    tile_id: TileId(id % 3),
                    padding: 0,
                    bounds: Default::default(),
                },
            }
            .into(),
        }
    }

    fn describe(primitive: &Primitive, normalize: bool) -> String {
        let mut primitive = primitive.clone();
        let order = if normalize { 0 } else { primitive.order() };
        match &mut primitive {
            Primitive::Shadow(value) => {
                value.order = order;
                format!("shadow:{value:?}")
            }
            Primitive::Quad(value) => {
                value.order = order;
                format!("quad:{value:?}")
            }
            Primitive::Path(value) => {
                value.order = order;
                if normalize {
                    value.id = PathId(0);
                }
                format!("path:{value:?}")
            }
            Primitive::Underline(value) => {
                value.order = order;
                format!("underline:{value:?}")
            }
            Primitive::MonochromeSprite(value) => {
                value.order = order;
                format!("mono:{value:?}")
            }
            Primitive::PolychromeSprite(value) => {
                value.order = order;
                format!("poly:{value:?}")
            }
            Primitive::Surface(value) => {
                value.order = order;
                format!("surface:{value:?}")
            }
        }
    }

    fn operations(scene: &Scene, normalize: bool) -> Vec<String> {
        scene
            .paint_operations
            .iter()
            .map(|operation| match operation {
                PaintOperation::Primitive(value) => describe(value, normalize),
                PaintOperation::StartLayer(bounds) => format!("start:{bounds:?}"),
                PaintOperation::EndLayer => "end".to_owned(),
            })
            .collect()
    }

    fn fingerprint(scene: &Scene) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
            operations(scene, false),
            scene.primitive_bounds,
            scene.layer_stack,
            scene.shadows,
            scene.quads,
            scene.paths,
            scene.underlines,
            scene.monochrome_sprites,
            scene.polychrome_sprites,
            scene.surfaces
        )
    }

    fn painted(scene: &Scene) -> Vec<(Bounds<ScaledPixels>, String)> {
        let mut primitives = Vec::new();
        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Shadows(values) => {
                    primitives.extend(values.iter().cloned().map(Primitive::from))
                }
                PrimitiveBatch::Quads(values) => {
                    primitives.extend(values.iter().cloned().map(Primitive::from))
                }
                PrimitiveBatch::Paths(values) => {
                    primitives.extend(values.iter().cloned().map(Primitive::from))
                }
                PrimitiveBatch::Underlines(values) => {
                    primitives.extend(values.iter().cloned().map(Primitive::from))
                }
                PrimitiveBatch::MonochromeSprites { sprites, .. } => {
                    primitives.extend(sprites.iter().cloned().map(Primitive::from))
                }
                PrimitiveBatch::PolychromeSprites { sprites, .. } => {
                    primitives.extend(sprites.iter().cloned().map(Primitive::from))
                }
                PrimitiveBatch::Surfaces(values) => {
                    primitives.extend(values.iter().cloned().map(Primitive::from))
                }
            }
        }
        primitives
            .iter()
            .map(|primitive| {
                (
                    primitive
                        .bounds()
                        .intersect(&primitive.content_mask().bounds),
                    describe(primitive, true),
                )
            })
            .collect()
    }

    fn equivalent(fast: &mut Scene, legacy: &mut Scene) -> Result<(), String> {
        require(
            operations(fast, true) == operations(legacy, true),
            "operation payload/layer stream changed",
        )?;
        require(
            fast.paths
                .iter()
                .map(|path| path.id)
                .eq(legacy.paths.iter().map(|path| path.id)),
            "replayed PathId assignment differs",
        )?;
        require(
            fast.paths
                .iter()
                .enumerate()
                .all(|(index, path)| path.id == PathId(index)),
            "replayed paths lack fresh contiguous ids",
        )?;
        fast.finish();
        legacy.finish();
        let actual = painted(fast);
        let expected = painted(legacy);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for (bounds, _) in actual.iter().chain(&expected) {
            xs.extend([bounds.origin.x.0, bounds.right().0]);
            ys.extend([bounds.origin.y.0, bounds.bottom().0]);
        }
        xs.sort_by(f32::total_cmp);
        xs.dedup();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        // Every elementary overlap cell is checked, not just operation order:
        // disjoint objects may gain constraints, but mixed-kind/alpha painter
        // order at any potentially painted location must match the old batches.
        for x in xs.windows(2) {
            for y in ys.windows(2) {
                let probe = point(
                    (x[0] + (x[1] - x[0]) * 0.5).into(),
                    (y[0] + (y[1] - y[0]) * 0.5).into(),
                );
                let active = |items: &[(Bounds<ScaledPixels>, String)]| {
                    items
                        .iter()
                        .filter(|(bounds, _)| bounds.contains(&probe))
                        .map(|(_, payload)| payload.clone())
                        .collect::<Vec<_>>()
                };
                if active(&actual) != active(&expected) {
                    return Err(format!("mixed painter order changed at {probe:?}"));
                }
            }
        }
        Ok(())
    }

    fn refused(scene: &mut Scene, range: Range<usize>, previous: &Scene) -> Result<(), String> {
        let before = fingerprint(scene);
        require(
            !scene.try_bulk_replay(range, previous),
            "ineligible replay accepted",
        )?;
        require(
            before == fingerprint(scene),
            "refused replay mutated scene/bounds-tree state",
        )
    }

    fn source(scale: f32, seed: u32) -> (Scene, Range<usize>) {
        let mut scene = Scene::default();
        let mask = bounds(-40.0, -40.0, 160.0, 160.0, scale);
        scene.insert_primitive(primitive(
            2,
            200,
            bounds(-8.0, -6.0, 19.0, 15.0, scale),
            mask,
        ));
        let start = scene.len();
        let mut state = u64::from(seed);
        for index in 0..24_u32 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = ((state >> 32) % 48) as f32 - 12.0;
            let y = ((state >> 40) % 32) as f32 - 8.0;
            if index == 8 {
                scene.push_layer(bounds(-14.0, -10.0, 70.0, 52.0, scale));
            }
            if index == 11 {
                scene.push_layer(bounds(-8.0, -5.0, 45.0, 40.0, scale));
            }
            if index == 15 {
                scene.pop_layer();
            }
            if index == 18 {
                scene.pop_layer();
            }
            let clip = if index % 5 == 0 {
                bounds(0.0, 0.0, 23.5, 17.25, scale)
            } else {
                mask
            };
            scene.insert_primitive(primitive(
                index as usize,
                index,
                bounds(x + 0.25, y + 0.5, 17.75, 13.25, scale),
                clip,
            ));
        }
        let end = scene.len();
        (scene, start..end)
    }

    fn verify_path_clone_value_semantics() -> Result<(), String> {
        fn bits<P: Clone + Debug + Default + PartialEq>(
            vertices: &[PathVertex<P>],
            raw: impl Fn(&P) -> f32,
        ) -> Vec<[u32; 8]> {
            vertices
                .iter()
                .map(|v| {
                    [
                        raw(&v.xy_position.x).to_bits(),
                        raw(&v.xy_position.y).to_bits(),
                        v.st_position.x.to_bits(),
                        v.st_position.y.to_bits(),
                        raw(&v.content_mask.bounds.origin.x).to_bits(),
                        raw(&v.content_mask.bounds.origin.y).to_bits(),
                        raw(&v.content_mask.bounds.size.width).to_bits(),
                        raw(&v.content_mask.bounds.size.height).to_bits(),
                    ]
                })
                .collect()
        }
        let mut original = Path::new(point(Pixels(-0.0), Pixels(0.25)));
        original.line_to(point(Pixels(2.5), Pixels(-3.75)));
        original.curve_to(
            point(Pixels(6.25), Pixels(4.5)),
            point(Pixels(1.125), Pixels(-0.0)),
        );
        let before = bits(&original.vertices, |v| v.0);
        let before_bounds = original.bounds;
        let mut edited = original.clone();
        edited.line_to(point(Pixels(123.0), Pixels(456.0)));
        require(
            bits(&original.vertices, |v| v.0) == before && original.bounds == before_bounds,
            "clone mutation changed original mesh or bounds",
        )?;
        require(
            edited.vertices.len() > original.vertices.len()
                && !std::ptr::eq(original.vertices.as_ptr(), edited.vertices.as_ptr()),
            "clone append did not detach mesh storage",
        )?;
        let mut shared_edit = original.clone();
        shared_edit.vertices = PathVertices::Shared(Arc::new(shared_edit.vertices.to_vec()));
        let shared_before = shared_edit.clone();
        shared_edit.line_to(point(Pixels(77.0), Pixels(88.0)));
        require(bits(&shared_before.vertices, |v| v.0) == before,
                "shared path mutation changed retained mesh")?;
        let retained = original.clone();
        original.line_to(point(Pixels(-70.0), Pixels(-80.0)));
        require(
            bits(&retained.vertices, |v| v.0) == before,
            "original mutation changed retained clone",
        )?;
        for factor in [0.5, 1.0, 1.25, 2.0, 3.0] {
            let scaled = retained.scale(factor);
            let expected = before
                .iter()
                .map(|old| {
                    let mut next = *old;
                    for i in [0, 1, 4, 5, 6, 7] {
                        next[i] = (f32::from_bits(old[i]) * factor).to_bits();
                    }
                    next
                })
                .collect::<Vec<_>>();
            require(
                bits(&scaled.vertices, |v| v.0) == expected,
                "scaled shared path differs from scalar coordinate/ST/mask bits",
            )?;
            let other = scaled.clone();
            require(
                std::ptr::eq(scaled.vertices.as_ptr(), other.vertices.as_ptr()),
                "scaled path clone copied mesh storage",
            )?;
        }
        Ok(())
    }

    pub(super) fn verify() -> Result<(), String> {
        verify_path_clone_value_semantics()?;
        for scale in [1.0, 2.0] {
            for seed in 1..=12 {
                let (previous, range) = source(scale, seed);
                let mut fast = Scene::default();
                let mut legacy = Scene::default();
                let mask = bounds(-40.0, -40.0, 160.0, 160.0, scale);
                let prefix = primitive(2, 100, bounds(-10.0, -6.0, 30.0, 19.0, scale), mask);
                fast.insert_primitive(prefix.clone());
                legacy.insert_primitive(prefix);
                require(
                    fast.try_bulk_replay(range.clone(), &previous),
                    "balanced mixed replay unexpectedly refused",
                )?;
                legacy.replay_legacy(range.clone(), &previous);
                // Another cached range, followed by a live translucent overlay.
                require(
                    fast.try_bulk_replay(range.clone(), &previous),
                    "second cached range refused",
                )?;
                legacy.replay_legacy(range, &previous);
                let suffix = primitive(1, 101, bounds(-3.0, -1.0, 44.0, 31.0, scale), mask);
                fast.insert_primitive(suffix.clone());
                legacy.insert_primitive(suffix);
                equivalent(&mut fast, &mut legacy)?;
                let mut next_fast = Scene::default();
                let mut next_legacy = Scene::default();
                require(
                    next_fast.try_bulk_replay(0..fast.len(), &fast),
                    "repeated fast replay refused",
                )?;
                next_legacy.replay_legacy(0..legacy.len(), &legacy);
                equivalent(&mut next_fast, &mut next_legacy)?;
                // Current external layer must preserve the original fallback.
                let mut fallback = Scene::default();
                let mut reference = Scene::default();
                fallback.push_layer(mask);
                reference.push_layer(mask);
                refused(&mut fallback, 0..next_fast.len(), &next_fast)?;
                fallback.replay(0..next_fast.len(), &next_fast);
                reference.replay_legacy(0..next_legacy.len(), &next_legacy);
                fallback.pop_layer();
                reference.pop_layer();
                equivalent(&mut fallback, &mut reference)?;

                // Fixed cached content across generations, with fresh live
                // prefix/overlay each frame. Rebased intervals must not grow
                // merely because the previous source was itself bulk replayed.
                let mut old_fast = fast;
                let mut old_legacy = legacy;
                let mut cached_fast = 1..old_fast.len() - 1;
                let mut cached_legacy = 1..old_legacy.len() - 1;
                let span = |scene: &Scene, range: Range<usize>| {
                    let mut orders = scene.paint_operations[range]
                        .iter()
                        .filter_map(|operation| match operation {
                            PaintOperation::Primitive(value) => Some(value.order()),
                            _ => None,
                        });
                    let first = orders.next().unwrap_or(0);
                    let (min, max) = orders.fold((first, first), |(min, max), order| {
                        (min.min(order), max.max(order))
                    });
                    max - min
                };
                let original_span = span(&old_fast, cached_fast.clone());
                for generation in 0..8 {
                    let mut current_fast = Scene::default();
                    let mut current_legacy = Scene::default();
                    let prefix = primitive(
                        2,
                        150,
                        bounds(-10.0 + generation as f32 * 0.5, -6.0, 30.0, 19.0, scale),
                        mask,
                    );
                    current_fast.insert_primitive(prefix.clone());
                    current_legacy.insert_primitive(prefix);
                    let start = current_fast.len();
                    require(
                        current_fast.try_bulk_replay(cached_fast, &old_fast),
                        "bulk-to-bulk cached generation refused",
                    )?;
                    current_legacy.replay_legacy(cached_legacy, &old_legacy);
                    let end = current_fast.len();
                    require(
                        span(&current_fast, start..end) == original_span,
                        "cached order span inflated across frames",
                    )?;
                    let overlay = primitive(
                        1,
                        151,
                        bounds(-3.0, generation as f32 * 0.25, 44.0, 31.0, scale),
                        mask,
                    );
                    current_fast.insert_primitive(overlay.clone());
                    current_legacy.insert_primitive(overlay);
                    equivalent(&mut current_fast, &mut current_legacy)?;
                    cached_fast = start..end;
                    cached_legacy = 1..current_legacy.len() - 1;
                    old_fast = current_fast;
                    old_legacy = current_legacy;
                }
            }
        }
        let area = bounds(0.0, 0.0, 20.0, 20.0, 1.0);
        let mut previous = Scene::default();
        previous.push_layer(area);
        previous.insert_primitive(primitive(1, 1, area, area));
        previous.pop_layer();
        let mut destination = Scene::default();
        refused(&mut destination, 1..2, &previous)?; // Old external layer.
        refused(&mut destination, 0..2, &previous)?; // Unbalanced contained layer.
        refused(&mut destination, 2..3, &previous)?; // End without start.
        refused(&mut destination, 4..5, &previous)?; // Invalid internal range.
        refused(&mut destination, 0..0, &previous)?; // Empty range safely falls back.
        let mut malformed_prefix = Scene::default();
        malformed_prefix
            .paint_operations
            .push(PaintOperation::EndLayer);
        malformed_prefix
            .paint_operations
            .push(PaintOperation::Primitive(primitive(1, 2, area, area)));
        refused(&mut destination, 1..2, &malformed_prefix)?;
        refused(&mut destination, 0..1, &malformed_prefix)?;
        let mut empty = Scene::default();
        empty
            .paint_operations
            .push(PaintOperation::Primitive(primitive(
                1,
                3,
                bounds(0.0, 0.0, 0.0, 10.0, 1.0),
                area,
            )));
        refused(&mut destination, 0..1, &empty)?;
        destination.replay(0..1, &empty);
        require(
            destination.len() == 0,
            "empty primitive replay gained a paint operation",
        )?;
        for invalid in [
            bounds(f32::NAN, 0.0, 1.0, 1.0, 1.0),
            bounds(0.0, 0.0, f32::INFINITY, 1.0, 1.0),
        ] {
            let mut source = Scene::default();
            source
                .paint_operations
                .push(PaintOperation::Primitive(primitive(1, 4, invalid, area)));
            refused(&mut destination, 0..1, &source)?;
        }
        let negative = bounds(0.0, 0.0, -1.0, 10.0, 1.0);
        let mut invalid_layer = Scene::default();
        invalid_layer
            .paint_operations
            .push(PaintOperation::StartLayer(negative));
        invalid_layer
            .paint_operations
            .push(PaintOperation::Primitive(primitive(1, 7, area, area)));
        invalid_layer
            .paint_operations
            .push(PaintOperation::EndLayer);
        refused(&mut destination, 0..3, &invalid_layer)?;
        for (extent, mask) in [
            (negative, area),
            (area, negative),
            (area, bounds(0.0, 0.0, f32::INFINITY, 10.0, 1.0)),
        ] {
            let mut invalid = Scene::default();
            invalid
                .paint_operations
                .push(PaintOperation::Primitive(primitive(1, 8, extent, mask)));
            refused(&mut destination, 0..1, &invalid)?;
        }
        let mut clipped = Scene::default();
        clipped.insert_primitive(primitive(1, 9, area, area));
        clipped
            .paint_operations
            .push(PaintOperation::Primitive(primitive(
                3,
                10,
                bounds(50.0, 50.0, 10.0, 10.0, 1.0),
                area,
            )));
        clipped.insert_primitive(primitive(2, 11, area, area));
        let mut fast_clipped = Scene::default();
        let mut old_clipped = Scene::default();
        require(
            fast_clipped.try_bulk_replay(0..clipped.len(), &clipped),
            "mixed visible/clipped replay refused",
        )?;
        old_clipped.replay_legacy(0..clipped.len(), &clipped);
        equivalent(&mut fast_clipped, &mut old_clipped)?;
        let mut overflow_union = Scene::default();
        for x in [-3.0e38, 2.0e38] {
            let extent = bounds(x, 0.0, 1.0e38, 1.0, 1.0);
            overflow_union
                .paint_operations
                .push(PaintOperation::Primitive(primitive(1, 5, extent, extent)));
        }
        refused(&mut destination, 0..2, &overflow_union)?;
        let before = fingerprint(&destination);
        require(
            destination
                .primitive_bounds
                .reserve(area, u32::MAX)
                .is_none(),
            "overflowing reservation accepted",
        )?;
        require(
            before == fingerprint(&destination),
            "overflowing reservation mutated tree",
        )?;
        require(
            destination
                .primitive_bounds
                .reserve(area, u32::MAX - 2)
                .is_some(),
            "valid near-limit reservation refused",
        )?;
        let mut normal = Scene::default();
        normal.insert_primitive(primitive(1, 6, area, area));
        refused(&mut destination, 0..normal.len(), &normal)?;
        Ok(())
    }
}

impl Path<Pixels> {
    /// Translate in logical pixels, then scale the retained path into a scene path.
    pub(crate) fn scale_with_offset(
        &self,
        factor: f32,
        offset: Point<Pixels>,
        content_mask: &ContentMask<Pixels>,
    ) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: Bounds {
                origin: (self.bounds.origin + offset).scale(factor),
                size: self.bounds.size.scale(factor),
            },
            content_mask: content_mask.scale(factor),
            vertices: PathVertices::Shared(Arc::new(self.vertices.iter()
                .map(|vertex| vertex.scale_with_offset(factor, offset)).collect())),
            start: (self.start + offset).scale(factor),
            current: (self.current + offset).scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }
}

impl PathVertex<Pixels> {
    fn scale_with_offset(&self, factor: f32, offset: Point<Pixels>) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: (self.xy_position + offset).scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}
