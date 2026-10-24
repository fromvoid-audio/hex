//! Named inventory of the library's public visual factories and meaningful variants.
//! Nonvisual numeric/text/color utilities are exercised through their controls.
//! Rich menu rows exercise action_surface; device/main status compositions use
//! the same level_meter gauge. TextInput covers query and source-value policies.
//! Musical editors and their project-bound hit regions remain with the app;
//! this catalog mounts their shared controls rather than a competing DAW model.
use super::workspace::Kind as WorkspaceKind;
use hex_widgets::{
    DeviceLayout, DeviceSectionSurface, DialLayout, DiscreteControlAppearance as Discrete,
    NumericControlAppearance as Appearance, NumericControlKind as Kind, NumericScale as Scale,
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Demo {
    Numeric(Kind, Appearance, DialLayout, Scale),
    Toggle(Discrete),
    Selector(Discrete),
    Bars,
    Graph,
    Color,
    Curve,
    Spectrum,
    Chrome(&'static str),
    Layout(DeviceLayout),
    Section(u8, DeviceSectionSurface),
    Frame(&'static str),
    Compositions,
    Workspace(WorkspaceKind),
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Item {
    pub name: &'static str,
    pub category: &'static str,
    pub demo: Demo,
}
pub(super) fn inventory() -> Vec<Item> {
    let mut items = Vec::new();
    let mut add = |category, name, demo| {
        items.push(Item {
            name,
            category,
            demo,
        })
    };
    for (name, appearance) in [
        ("standard knob", Appearance::Standard),
        ("compact dial", Appearance::CompactDial),
        ("micro dial", Appearance::MicroDial),
        ("wide micro dial", Appearance::WideMicroDial),
    ] {
        add(
            "numeric",
            name,
            Demo::Numeric(
                Kind::Knob,
                appearance,
                DialLayout::ReadoutGap,
                Scale::Linear,
            ),
        );
    }
    for (name, kind, appearance, layout, scale) in [
        (
            "inline value",
            Kind::HorizontalSlider,
            Appearance::InlineValue,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "horizontal slider",
            Kind::HorizontalSlider,
            Appearance::Standard,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "vertical slider",
            Kind::VerticalSlider,
            Appearance::Standard,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "compact slider",
            Kind::HorizontalSlider,
            Appearance::CompactSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "filled slider",
            Kind::HorizontalSlider,
            Appearance::FilledSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "device meters",
            Kind::VerticalSlider,
            Appearance::DeviceMeters,
            DialLayout::ReadoutGap,
            Scale::LinearAmplitudeDecibels,
        ),
        (
            "bottom gap dial",
            Kind::Knob,
            Appearance::CompactDial,
            DialLayout::BottomGap,
            Scale::Linear,
        ),
        (
            "pan dial · bipolar",
            Kind::Knob,
            Appearance::Standard,
            DialLayout::Pan,
            Scale::Bipolar,
        ),
        (
            "frequency dial · logarithmic",
            Kind::Knob,
            Appearance::CompactDial,
            DialLayout::ReadoutGap,
            Scale::Logarithmic,
        ),
    ] {
        add(
            "numeric",
            name,
            Demo::Numeric(kind, appearance, layout, scale),
        );
    }
    for (name, appearance) in [
        ("standard toggle", Discrete::Standard),
        ("compact button", Discrete::CompactButton),
        ("compact square", Discrete::CompactSquare),
        ("micro square", Discrete::MicroSquare),
        ("compact icon button", Discrete::CompactIconButton),
    ] {
        add("discrete", name, Demo::Toggle(appearance));
    }
    for (name, appearance) in [
        ("standard selector", Discrete::Standard),
        ("compact selector", Discrete::CompactSelector),
        ("compact segments", Discrete::CompactSegments),
    ] {
        add("discrete", name, Demo::Selector(appearance));
    }
    for (name, demo) in [
        ("bar editor", Demo::Bars),
        ("response graph", Demo::Graph),
        ("color picker", Demo::Color),
        ("curve plot", Demo::Curve),
        ("layered spectrum plot", Demo::Spectrum),
    ] {
        add("graphics", name, demo);
    }
    for name in [
        "action",
        "device action",
        "device badge",
        "device caption",
        "device divider",
        "device focus target",
        "device identity marker",
        "device inline caption",
        "device segment",
        "panel tab",
    ] {
        add("chrome", name, Demo::Chrome(name));
    }
    for (name, layout) in [
        ("row", DeviceLayout::Row),
        ("column", DeviceLayout::Column),
        ("wrapped", DeviceLayout::Wrapped),
        ("grid · two columns", DeviceLayout::Grid(2)),
        ("grid · three columns", DeviceLayout::Grid(3)),
    ] {
        add("layout", name, Demo::Layout(layout));
    }
    for (density, names) in [
        (
            0,
            ["section · surround", "section · rail", "section · display"],
        ),
        (
            1,
            [
                "compact section · surround",
                "compact section · rail",
                "compact section · display",
            ],
        ),
        (
            2,
            [
                "dense section · surround",
                "dense section · rail",
                "dense section · display",
            ],
        ),
    ] {
        for (name, surface) in names.into_iter().zip([
            DeviceSectionSurface::Surround,
            DeviceSectionSurface::Rail,
            DeviceSectionSurface::Display,
        ]) {
            add("layout", name, Demo::Section(density, surface));
        }
    }
    for name in [
        "device split",
        "device body",
        "windowed body",
        "compact row · idle",
        "compact row · active",
        "grouped row · idle",
        "grouped row · active",
        "device rail",
        "device display",
        "device frame",
        "device header",
    ] {
        add("layout", name, Demo::Frame(name));
    }
    // Shared text/gauge primitives plus actual app-owned popup/status compositions.
    // Their fixtures contain only local values and typed UI callbacks.
    for (name, kind) in [
        (
            "text input · value",
            WorkspaceKind::Text(hex_widgets::TextInputKind::Value),
        ),
        (
            "text input · search",
            WorkspaceKind::Text(hex_widgets::TextInputKind::Search),
        ),
        (
            "popup menu · choices and swatches",
            WorkspaceKind::Menu(false),
        ),
        ("popup menu · icon grid", WorkspaceKind::Menu(true)),
        ("level meter", WorkspaceKind::LevelMeter),
        ("output sample-peak status", WorkspaceKind::OutputMeter),
        ("scrolling list", WorkspaceKind::ScrollingList),
    ] {
        add("workspace", name, Demo::Workspace(kind));
    }
    for (name, button) in [
        ("play button", "play"),
        ("track activator", "track"),
        ("stop button", "stop"),
        ("record button", "record"),
        ("mute button", "mute"),
        ("solo button", "solo"),
        ("arm button", "arm"),
        ("fold disclosure", "fold"),
    ] {
        add(
            "workspace",
            name,
            Demo::Workspace(WorkspaceKind::Button(button)),
        );
    }
    add(
        "compositions",
        "device compositions · 12 fixtures",
        Demo::Compositions,
    );
    items
}
pub(super) const CATEGORIES: [&str; 7] = [
    "numeric",
    "discrete",
    "graphics",
    "chrome",
    "layout",
    "workspace",
    "compositions",
];
