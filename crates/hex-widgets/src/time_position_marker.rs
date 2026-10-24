//! Shared point/range position triangles. Callers supply normalized visible
//! positions, palette and meaning; this surface owns no time model or input.
use gpui::{Div, PathBuilder, SharedString, canvas, div, point, prelude::*, px, relative, rgb};

pub const TIME_POSITION_MARKER_HEIGHT: f32 = 6.0;
const HALF_WIDTH: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeMarkerEndpoint {
    Point,
    Start,
    End,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeMarkerEdge {
    Top,
    Bottom,
}

/// The tip lies exactly on `position`. Range halves extend outside the range;
/// coincident halves form the same full triangle as a point cursor. Offscreen
/// and nonfinite positions paint nothing, rather than pinning a false endpoint.
pub fn time_position_marker(
    id: SharedString,
    position: f32,
    endpoint: TimeMarkerEndpoint,
    edge: TimeMarkerEdge,
    color: u32,
) -> Option<Div> {
    if !position.is_finite() || !(0.0..=1.0).contains(&position) {
        return None;
    }
    let width = if endpoint == TimeMarkerEndpoint::Point {
        HALF_WIDTH * 2.0
    } else {
        HALF_WIDTH
    };
    let tip = if endpoint == TimeMarkerEndpoint::End {
        0.0
    } else {
        HALF_WIDTH
    };
    Some(
        div()
            .debug_selector(move || id.to_string())
            .absolute()
            .left(relative(position))
            .ml(px(-tip))
            .w(px(width))
            .h(px(TIME_POSITION_MARKER_HEIGHT))
            .when(edge == TimeMarkerEdge::Top, |marker| marker.top_0())
            .when(edge == TimeMarkerEdge::Bottom, |marker| marker.bottom_0())
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let (base, apex) = if edge == TimeMarkerEdge::Top {
                            (bounds.top(), bounds.bottom())
                        } else {
                            (bounds.bottom(), bounds.top())
                        };
                        let mut path = PathBuilder::fill();
                        path.move_to(point(bounds.left(), base));
                        path.line_to(point(bounds.right(), base));
                        path.line_to(point(bounds.left() + px(tip), apex));
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(color));
                        }
                    },
                )
                .size_full(),
            ),
    )
}
