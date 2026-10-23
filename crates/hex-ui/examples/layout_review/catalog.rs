//! Named inventory of the library's public visual factories and meaningful variants.
//! Nonvisual numeric/text/color utilities are exercised through their controls.
use hex_widgets::{
    DeviceLayout, DeviceSectionSurface, DialLayout, DiscreteControlAppearance as Discrete,
    NumericControlAppearance as Appearance, NumericControlKind as Kind, NumericDragAxis as Axis,
    NumericScale as Scale,
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Demo {
    Numeric(Kind, Appearance, DialLayout, Scale, Axis),
    Toggle(Discrete),
    Selector(Discrete),
    Bars,
    Graph,
    Color,
    Curve,
    Transfer,
    Spectrum,
    Chrome(&'static str),
    Layout(DeviceLayout),
    Section(u8, DeviceSectionSurface),
    Frame(&'static str),
    Compositions,
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
        ("mini dial", Appearance::MiniDial),
        ("micro dial", Appearance::MicroDial),
        ("wide micro dial", Appearance::WideMicroDial),
        ("inline value", Appearance::InlineValue),
    ] {
        add(
            "numeric",
            name,
            Demo::Numeric(
                Kind::Knob,
                appearance,
                DialLayout::ReadoutGap,
                Scale::Linear,
                Axis::Vertical,
            ),
        );
    }
    for (name, kind, appearance, layout, scale) in [
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
            Demo::Numeric(
                kind,
                appearance,
                layout,
                scale,
                if kind == Kind::HorizontalSlider {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                },
            ),
        );
    }
    add(
        "numeric",
        "horizontal scrub dial",
        Demo::Numeric(
            Kind::Knob,
            Appearance::CompactDial,
            DialLayout::ReadoutGap,
            Scale::Linear,
            Axis::Horizontal,
        ),
    );
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
        ("transfer plot", Demo::Transfer),
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
    add(
        "compositions",
        "device compositions · 12 fixtures",
        Demo::Compositions,
    );
    items
}
pub(super) const CATEGORIES: [&str; 6] = [
    "numeric",
    "discrete",
    "graphics",
    "chrome",
    "layout",
    "compositions",
];
