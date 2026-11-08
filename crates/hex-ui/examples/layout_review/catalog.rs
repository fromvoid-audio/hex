//! Named inventory of the library's public visual factories and meaningful variants.
//! Nonvisual numeric/text/color utilities are exercised through their controls.
//! Rich menu rows exercise action_surface; device/main status compositions use
//! the same level_meter gauge. TextInput covers query and source-value policies.
//! Musical editors and their project-bound hit regions remain with the app;
//! this catalog mounts their shared controls rather than a competing DAW model.
use super::workspace::{Kind as WorkspaceKind, MeterChannels, RoundActionKind};
use hex_widgets::{
    DeviceLayout, DeviceSectionSurface, DialLayout, NumericControlKind as Kind,
    NumericControlPresentation as Appearance, NumericScale as Scale, SelectorAppearance,
    ToggleAppearance,
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Demo {
    Numeric(Appearance, DialLayout, Scale),
    Toggle(ToggleAppearance),
    Selector(SelectorAppearance),
    Bars,
    Rectangle(bool),
    TimeRange,
    Graph,
    Color,
    Curve,
    Spectrum,
    Waveform(bool),
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
/// Catalog pages follow the visual control kind within an owning family.
/// Appearances, layouts and scales of the same kind share a page.
/// Multi-control fixtures remain explicitly identified as compositions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Page {
    pub name: &'static str,
    pub category: &'static str,
    pub owner: &'static str,
}
impl Item {
    pub fn page(self) -> Page {
        let (name, category, owner) = match self.demo {
            Demo::Numeric(presentation, ..) => (
                match presentation.kind() {
                    Kind::Knob => "knob",
                    Kind::HorizontalSlider => "horizontal slider",
                    Kind::VerticalSlider => "vertical slider",
                },
                "numeric",
                "numericcontrol",
            ),
            Demo::Toggle(_) => ("toggle", "discrete", "toggle"),
            Demo::Selector(_) => ("selector", "discrete", "selector"),
            Demo::Rectangle(_) => ("rectangle", "graphics", "rectangle"),
            Demo::TimeRange => ("time selection", "graphics", "range_selection"),
            Demo::Bars => ("bar editor", "graphics", "bareditor"),
            Demo::Graph => ("response graph", "graphics", "responsegraph"),
            Demo::Color => ("color picker", "graphics", "colorpicker"),
            Demo::Curve => ("curve plot", "graphics", "curve_plot"),
            Demo::Spectrum => ("spectrum plot", "graphics", "layered_spectrum_plot"),
            Demo::Waveform(_) => ("waveform", "graphics", "waveform_plot"),
            Demo::Chrome("action" | "device action" | "device segment")
            | Demo::Workspace(WorkspaceKind::Button(_) | WorkspaceKind::RoundAction(_)) => {
                ("action surface", "chrome", "action_surface")
            }
            Demo::Chrome("device caption" | "device inline caption") => {
                ("device caption", "chrome", "device_caption")
            }
            Demo::Chrome(name) => (name, "chrome", name),
            Demo::Layout(_) => ("device layout", "layout", "device_layout"),
            Demo::Section(..) => ("device section", "layout", "section"),
            Demo::Frame(_) => (
                "structural compositions",
                "compositions",
                "composed layout and chrome",
            ),
            Demo::Compositions => ("device compositions", "compositions", "device_composition"),
            Demo::Workspace(WorkspaceKind::Splitter(_)) => {
                ("captured splitter", "layout", "splitter")
            }
            Demo::Workspace(WorkspaceKind::Text(_)) => ("text input", "workspace", "textinput"),
            Demo::Workspace(WorkspaceKind::Menu(_)) => (
                "popup menu composition",
                "compositions",
                "context_menu::popup",
            ),
            Demo::Workspace(WorkspaceKind::OutputMeter) => (
                "output status composition",
                "compositions",
                "output_meter::render",
            ),
            Demo::Workspace(WorkspaceKind::MeterIndicator(_)) => {
                ("meter indicator", "workspace", "meter_indicator")
            }
            Demo::Workspace(WorkspaceKind::LevelMeter) => {
                ("level meter", "workspace", "level_meter")
            }
            Demo::Workspace(WorkspaceKind::TimeMarkers) => (
                "time selection markers",
                "workspace",
                "time_position_marker",
            ),
            Demo::Workspace(WorkspaceKind::ScrollingList) => (
                "scrolling list composition",
                "compositions",
                "scroll container and action rows",
            ),
        };
        Page {
            name,
            category,
            owner,
        }
    }
    pub fn subgroup(self) -> &'static str {
        match self.demo {
            Demo::Waveform(_) => "channel configurations",
            Demo::Numeric(p, ..) if p.kind() == Kind::Knob => "knob configurations",
            Demo::Numeric(p, ..) if p.kind() == Kind::HorizontalSlider => {
                "horizontal slider configurations"
            }
            Demo::Numeric(p, ..) if p.kind() == Kind::VerticalSlider => {
                "vertical slider configurations"
            }
            Demo::Workspace(WorkspaceKind::Button(_)) => "glyph and state configurations",
            Demo::Workspace(WorkspaceKind::RoundAction(_)) => "round momentary configurations",
            Demo::Workspace(WorkspaceKind::Splitter(_)) => "movement axis configurations",
            Demo::Frame(_) => "composed layout fixtures",
            Demo::Compositions => "device fixtures",
            _ => "configurations",
        }
    }
    pub fn configuration(self) -> String {
        match self.demo {
            Demo::Numeric(presentation, layout, scale) => {
                format!("presentation: {presentation:?} · layout: {layout:?} · scale: {scale:?}")
            }
            Demo::Toggle(appearance) => format!("appearance: {appearance:?}"),
            Demo::Selector(appearance) => format!("appearance: {appearance:?}"),
            Demo::Waveform(stereo) => {
                format!("channels: {}", if stereo { "stereo" } else { "mono" })
            }
            Demo::Layout(layout) => format!("layout: {layout:?}"),
            Demo::Section(density, surface) => format!(
                "density: {} · surface: {surface:?}",
                match density {
                    0 => "standard",
                    1 => "compact",
                    _ => "dense",
                }
            ),
            _ => format!("configuration: {}", self.name),
        }
    }
}
pub(super) fn pages(items: &[Item]) -> Vec<Page> {
    let mut pages = Vec::new();
    for item in items {
        debug_assert!(CATEGORIES.contains(&item.category));
        let page = item.page();
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    pages
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
        ("standard knob", Appearance::Knob),
        ("compact dial", Appearance::CompactDial),
        ("micro dial", Appearance::MicroDial),
        ("wide micro dial", Appearance::WideMicroDial),
    ] {
        add(
            "numeric",
            name,
            Demo::Numeric(appearance, DialLayout::ReadoutGap, Scale::Linear),
        );
    }
    for (name, appearance, layout, scale) in [
        (
            "inline value",
            Appearance::InlineValue,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "horizontal slider",
            Appearance::HorizontalSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "vertical slider",
            Appearance::VerticalSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "compact slider",
            Appearance::CompactSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "labeled slider",
            Appearance::LabeledSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "filled slider",
            Appearance::FilledSlider,
            DialLayout::ReadoutGap,
            Scale::Linear,
        ),
        (
            "bottom gap dial",
            Appearance::CompactDial,
            DialLayout::BottomGap,
            Scale::Linear,
        ),
        (
            "pan dial · bipolar",
            Appearance::Knob,
            DialLayout::Pan,
            Scale::Bipolar,
        ),
        (
            "frequency dial · logarithmic",
            Appearance::CompactDial,
            DialLayout::ReadoutGap,
            Scale::Logarithmic,
        ),
    ] {
        add("numeric", name, Demo::Numeric(appearance, layout, scale));
    }
    for (name, appearance) in [
        ("standard toggle", ToggleAppearance::Standard),
        ("compact button", ToggleAppearance::CompactButton),
        ("compact square", ToggleAppearance::CompactSquare),
        ("micro square", ToggleAppearance::MicroSquare),
        ("compact icon button", ToggleAppearance::CompactIconButton),
    ] {
        add("discrete", name, Demo::Toggle(appearance));
    }
    for (name, appearance) in [
        ("standard selector", SelectorAppearance::Standard),
        ("compact selector", SelectorAppearance::CompactSelector),
        ("compact segments", SelectorAppearance::CompactSegments),
    ] {
        add("discrete", name, Demo::Selector(appearance));
    }
    for (name, demo) in [
        ("bar editor", Demo::Bars),
        ("clip rectangle", Demo::Rectangle(false)),
        ("section rectangle", Demo::Rectangle(true)),
        ("time selection", Demo::TimeRange),
        ("response graph", Demo::Graph),
        ("color picker", Demo::Color),
        ("curve plot", Demo::Curve),
        ("layered spectrum plot", Demo::Spectrum),
        ("mono waveform", Demo::Waveform(false)),
        ("stereo waveform", Demo::Waveform(true)),
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
        ("time selection markers", WorkspaceKind::TimeMarkers),
        (
            "meter indicator · mono",
            WorkspaceKind::MeterIndicator(MeterChannels::Mono),
        ),
        (
            "meter indicator · stereo",
            WorkspaceKind::MeterIndicator(MeterChannels::Stereo),
        ),
        (
            "round labeled action",
            WorkspaceKind::RoundAction(RoundActionKind::Label),
        ),
        (
            "round icon action",
            WorkspaceKind::RoundAction(RoundActionKind::Icon),
        ),
        (
            "splitter · horizontal",
            WorkspaceKind::Splitter(hex_widgets::SplitterAxis::Horizontal),
        ),
        (
            "splitter · vertical",
            WorkspaceKind::Splitter(hex_widgets::SplitterAxis::Vertical),
        ),
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

#[cfg(test)]
mod grouping_tests {
    use super::*;
    #[test]
    fn every_configuration_has_one_named_page_and_unique_identity() {
        let items = inventory();
        let pages = pages(&items);
        let names: std::collections::HashSet<_> = items.iter().map(|item| item.name).collect();
        assert_eq!(names.len(), items.len());
        let page_names: std::collections::HashSet<_> = pages.iter().map(|page| page.name).collect();
        assert_eq!(page_names.len(), pages.len());
        assert!(pages.len() < items.len());
        for item in &items {
            assert_eq!(
                pages.iter().filter(|page| **page == item.page()).count(),
                1,
                "{}",
                item.name
            );
            assert!(!item.configuration().is_empty());
            assert!(CATEGORIES.contains(&item.page().category));
        }
        for category in CATEGORIES {
            assert!(
                pages.iter().any(|page| page.category == category),
                "{category}"
            );
        }
    }
    #[test]
    fn appearance_wrappers_share_their_actual_base_and_compositions_stay_explicit() {
        let items = inventory();
        let page = |name| items.iter().find(|item| item.name == name).unwrap().page();
        for names in [
            vec![
                "inline value",
                "horizontal slider",
                "compact slider",
                "filled slider",
                "labeled slider",
            ],
            vec![
                "standard knob",
                "compact dial",
                "micro dial",
                "wide micro dial",
                "bottom gap dial",
                "pan dial · bipolar",
                "frequency dial · logarithmic",
            ],
        ] {
            for &name in &names {
                assert_eq!(page(name), page(names[0]), "{name}");
                assert_eq!(page(name).category, "numeric");
                assert_eq!(page(name).owner, "numericcontrol");
            }
        }
        let numeric_pages: Vec<_> = pages(&items)
            .into_iter()
            .filter(|page| page.category == "numeric")
            .collect();
        assert_eq!(
            numeric_pages
                .iter()
                .map(|page| page.name)
                .collect::<Vec<_>>(),
            ["knob", "horizontal slider", "vertical slider"]
        );
        for name in [
            "device action",
            "device segment",
            "play button",
            "fold disclosure",
            "round labeled action",
            "round icon action",
        ] {
            assert_eq!(page(name), page("action"), "{name}");
        }
        assert_eq!(page("device caption"), page("device inline caption"));
        assert_eq!(page("standard toggle"), page("compact button"));
        assert_ne!(page("standard toggle"), page("standard selector"));
        assert_eq!(page("text input · value"), page("text input · search"));
        assert_eq!(page("row"), page("grid · three columns"));
        assert_eq!(page("section · surround"), page("dense section · display"));
        assert_eq!(page("device body").category, "compositions");
        assert_eq!(page("popup menu · icon grid").category, "compositions");
        assert_eq!(page("output sample-peak status").category, "compositions");
        assert_ne!(page("output sample-peak status"), page("level meter"));
        assert_eq!(
            page("meter indicator · mono"),
            page("meter indicator · stereo")
        );
        assert_ne!(page("meter indicator · mono"), page("level meter"));
    }
}
