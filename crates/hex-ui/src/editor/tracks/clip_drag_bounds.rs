use super::drag::ClipSourceSpan;
use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, StyleRefinement, Styled, Window,
};

/// Observe the resting clip's resolved rectangle through its existing element.
/// The move strip excludes trim handles and cannot supply the clip's thirds.
/// GPUI passes the border box, including the leading stroke. The trailing
/// stroke is a separate paint layer outside that box.
pub(super) struct ClipDragBounds<E> {
    body: E,
    source_span: Option<ClipSourceSpan>,
}

impl<E> ClipDragBounds<E> {
    pub(super) fn new(body: E, source_span: Option<ClipSourceSpan>) -> Self {
        Self { body, source_span }
    }
}

impl<E: Styled> Styled for ClipDragBounds<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.body.style()
    }
}

impl<E: Element> IntoElement for ClipDragBounds<E> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<E: Element> Element for ClipDragBounds<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.body.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.body.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.body.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(span) = &self.source_span {
            span.set(Some((
                f32::from(bounds.left()),
                f32::from(bounds.size.width),
            )));
        }
        self.body
            .prepaint(id, inspector_id, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.body
            .paint(id, inspector_id, bounds, layout, prepaint, window, cx);
    }
}
