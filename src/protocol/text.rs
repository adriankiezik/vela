//! Typed text-component model and its network-NBT encoder.
//!
//! A chat component on the wire is an NBT compound. Its *content* fields, its
//! *style* fields, and its `extra` sibling list all live at the same level of
//! that compound — exactly as the decompiled `ComponentSerialization` codec
//! encodes a `MutableComponent` (content `MapCodec` ⊕ `extra` list ⊕
//! `Style.Serializer.MAP_CODEC`). This module mirrors that structure as a real
//! Rust data model and renders it with [`TextComponent::to_nbt`]. No Mojang code
//! is copied; the field names and shapes are transcribed from the 26.2 codecs:
//!
//!   * `Style.Serializer.MAP_CODEC` — style keys are **snake_case**:
//!     `color`, `shadow_color`, `bold`, `italic`, `underlined`, `strikethrough`,
//!     `obfuscated`, `click_event`, `hover_event`, `insertion`, `font`. Boolean
//!     flags encode as `Codec.BOOL` → NBT byte; `shadow_color` is an ARGB int.
//!   * `TextColor.CODEC` — a string, either a named color (`"green"`) or
//!     `#rrggbb` for an arbitrary RGB value.
//!   * `ClickEvent` / `HoverEvent` — a compound dispatched on an `action`
//!     string, with the variant's own fields inlined (e.g. `run_command` carries
//!     `command`, `copy_to_clipboard` carries `value`, `show_text` carries a
//!     `value` component).
//!   * content `MapCodec`s — `PlainTextContents` (`text`), `TranslatableContents`
//!     (`translate` (+ optional `fallback`, `with`)), `KeybindContents`
//!     (`keybind`), `ScoreContents` (`score{name,objective}`), `SelectorContents`
//!     (`selector` (+ optional `separator`)).
//!
//! `with` and `extra` are homogeneous NBT lists of component compounds, matching
//! the wire (a translation arg that is a plain literal would be collapsed to a
//! bare string by vanilla, but a `{text:…}` compound decodes identically).
//!
//! Dead-code note: this typed model is broader than the handful of builders the
//! `sim` shim currently drives, so unused-but-intentional API surface carries a
//! targeted `#[allow(dead_code)]` rather than a module-wide blanket — that way a
//! genuinely orphaned item still surfaces as a warning as the model grows.

use std::num::NonZeroU32;

use crate::protocol::nbt::Nbt;

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

/// One of the 16 vanilla named colors (`ChatFormatting`'s color entries, in
/// ordinal order). These are the only names `TextColor.parseColor` accepts as a
/// name — any other string the client rejects, so the type system enforces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    DarkBlue,
    DarkGreen,
    DarkAqua,
    DarkRed,
    DarkPurple,
    Gold,
    Gray,
    DarkGray,
    Blue,
    Green,
    Aqua,
    Red,
    LightPurple,
    Yellow,
    White,
}

impl NamedColor {
    /// The lowercase wire name (e.g. `dark_blue`), as `ChatFormatting.getName`.
    pub fn name(self) -> &'static str {
        match self {
            NamedColor::Black => "black",
            NamedColor::DarkBlue => "dark_blue",
            NamedColor::DarkGreen => "dark_green",
            NamedColor::DarkAqua => "dark_aqua",
            NamedColor::DarkRed => "dark_red",
            NamedColor::DarkPurple => "dark_purple",
            NamedColor::Gold => "gold",
            NamedColor::Gray => "gray",
            NamedColor::DarkGray => "dark_gray",
            NamedColor::Blue => "blue",
            NamedColor::Green => "green",
            NamedColor::Aqua => "aqua",
            NamedColor::Red => "red",
            NamedColor::LightPurple => "light_purple",
            NamedColor::Yellow => "yellow",
            NamedColor::White => "white",
        }
    }

    /// Resolve a wire name to a named color, or `None` if it isn't one of the 16.
    pub fn from_name(name: &str) -> Option<NamedColor> {
        Some(match name {
            "black" => NamedColor::Black,
            "dark_blue" => NamedColor::DarkBlue,
            "dark_green" => NamedColor::DarkGreen,
            "dark_aqua" => NamedColor::DarkAqua,
            "dark_red" => NamedColor::DarkRed,
            "dark_purple" => NamedColor::DarkPurple,
            "gold" => NamedColor::Gold,
            "gray" => NamedColor::Gray,
            "dark_gray" => NamedColor::DarkGray,
            "blue" => NamedColor::Blue,
            "green" => NamedColor::Green,
            "aqua" => NamedColor::Aqua,
            "red" => NamedColor::Red,
            "light_purple" => NamedColor::LightPurple,
            "yellow" => NamedColor::Yellow,
            "white" => NamedColor::White,
            _ => return None,
        })
    }
}

/// A text color — one of the 16 named vanilla colors or an arbitrary 24-bit RGB
/// value. Mirrors `net.minecraft.network.chat.TextColor`, which serializes as
/// the color name when it has one and `#rrggbb` otherwise. Using [`NamedColor`]
/// (rather than a free string) means an invalid name can't reach the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// One of the 16 named colors.
    Named(NamedColor),
    /// A 24-bit RGB value, rendered as `#rrggbb`.
    Rgb(u32),
}

impl Color {
    /// A named color, e.g. `Color::named(NamedColor::Green)`.
    #[allow(dead_code)] // typed-model convenience; sim shim builds colors via `parse`
    pub fn named(color: NamedColor) -> Color {
        Color::Named(color)
    }

    /// An arbitrary RGB color (low 24 bits used).
    #[allow(dead_code)] // typed-model convenience; not yet driven by a call site
    pub fn rgb(value: u32) -> Color {
        Color::Rgb(value & 0xFF_FFFF)
    }

    /// Parse a color the way `TextColor.parseColor` does: `#rrggbb` hex, else one
    /// of the 16 named colors. `None` for an unparseable `#` value or unknown
    /// name (the client would reject either).
    pub fn parse(s: &str) -> Option<Color> {
        if let Some(hex) = s.strip_prefix('#') {
            return u32::from_str_radix(hex, 16)
                .ok()
                .map(|v| Color::Rgb(v & 0xFF_FFFF));
        }
        NamedColor::from_name(s).map(Color::Named)
    }

    /// The wire string: the name, or `#rrggbb` (uppercase hex, as `String.format`
    /// `"#%06X"` produces in `TextColor.formatValue`).
    pub fn serialize(&self) -> String {
        match self {
            Color::Named(name) => name.name().to_string(),
            Color::Rgb(value) => format!("#{:06X}", value & 0xFF_FFFF),
        }
    }
}

// ---------------------------------------------------------------------------
// Click / hover events
// ---------------------------------------------------------------------------

/// A click event (`net.minecraft.network.chat.ClickEvent`). Only the actions a
/// server is allowed to send and that need no extra registries are modelled.
// Several variants aren't wired to a call site yet; the sim shim only builds
// `CopyToClipboard`. Kept as intentional API surface.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum ClickEvent {
    OpenUrl(String),
    RunCommand(String),
    SuggestCommand(String),
    CopyToClipboard(String),
    /// `change_page` — vanilla `ChangePage.CODEC` uses `ExtraCodecs.POSITIVE_INT`
    /// (≥1) and the client rejects page 0, so the page is a [`NonZeroU32`].
    ChangePage(NonZeroU32),
}

impl ClickEvent {
    /// Encode as the dispatched `{action, …}` compound.
    pub fn to_nbt(&self) -> Nbt {
        let (action, field, value) = match self {
            ClickEvent::OpenUrl(url) => ("open_url", "url", Nbt::string(url)),
            ClickEvent::RunCommand(cmd) => ("run_command", "command", Nbt::string(cmd)),
            ClickEvent::SuggestCommand(cmd) => ("suggest_command", "command", Nbt::string(cmd)),
            ClickEvent::CopyToClipboard(v) => ("copy_to_clipboard", "value", Nbt::string(v)),
            ClickEvent::ChangePage(p) => ("change_page", "page", Nbt::Int(p.get() as i32)),
        };
        Nbt::compound([("action", Nbt::string(action)), (field, value)])
    }
}

/// A hover event (`net.minecraft.network.chat.HoverEvent`). `show_item` /
/// `show_entity` are omitted — they need the item-stack / entity codecs.
#[allow(dead_code)] // the sim shim builds show_text as raw Nbt; typed form unused yet
#[derive(Clone, Debug, PartialEq)]
pub enum HoverEvent {
    ShowText(Box<TextComponent>),
}

impl HoverEvent {
    /// Encode as the dispatched `{action, …}` compound.
    pub fn to_nbt(&self) -> Nbt {
        match self {
            HoverEvent::ShowText(component) => Nbt::compound([
                ("action", Nbt::string("show_text")),
                ("value", component.to_nbt()),
            ]),
        }
    }
}

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

/// Component style — mirrors `net.minecraft.network.chat.Style`. Every field is
/// optional (absent = inherit from the parent); only set fields are encoded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub color: Option<Color>,
    pub shadow_color: Option<i32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underlined: Option<bool>,
    pub strikethrough: Option<bool>,
    pub obfuscated: Option<bool>,
    pub click_event: Option<ClickEvent>,
    pub hover_event: Option<HoverEvent>,
    pub insertion: Option<String>,
    pub font: Option<String>,
}

impl Style {
    /// True when no field is set — the component compound then needs no style
    /// keys at all.
    fn is_empty(&self) -> bool {
        *self == Style::default()
    }

    /// Append this style's set fields to a component compound's entry vector,
    /// in the order `Style.Serializer.MAP_CODEC` declares them.
    fn write_into(&self, fields: &mut Vec<(String, Nbt)>) {
        if let Some(color) = &self.color {
            fields.push(("color".into(), Nbt::string(color.serialize())));
        }
        if let Some(shadow) = self.shadow_color {
            fields.push(("shadow_color".into(), Nbt::Int(shadow)));
        }
        for (key, flag) in [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underlined", self.underlined),
            ("strikethrough", self.strikethrough),
            ("obfuscated", self.obfuscated),
        ] {
            if let Some(value) = flag {
                fields.push((key.into(), Nbt::bool(value)));
            }
        }
        if let Some(click) = &self.click_event {
            fields.push(("click_event".into(), click.to_nbt()));
        }
        if let Some(hover) = &self.hover_event {
            fields.push(("hover_event".into(), hover.to_nbt()));
        }
        if let Some(insertion) = &self.insertion {
            fields.push(("insertion".into(), Nbt::string(insertion)));
        }
        if let Some(font) = &self.font {
            fields.push(("font".into(), Nbt::string(font)));
        }
    }
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

/// The content of a component — one of the `ComponentContents` variants whose
/// map fields are inlined into the component compound.
// Only `Text`/`Translatable` are driven so far; the rest are modelled for parity.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// `PlainTextContents` → `{text}`.
    Text(String),
    /// `TranslatableContents` → `{translate, fallback?, with?}`.
    Translatable {
        key: String,
        fallback: Option<String>,
        with: Vec<TextComponent>,
    },
    /// `KeybindContents` → `{keybind}`.
    Keybind(String),
    /// `ScoreContents` → `{score: {name, objective}}`.
    Score { name: String, objective: String },
    /// `SelectorContents` → `{selector, separator?}`.
    Selector {
        selector: String,
        separator: Option<Box<TextComponent>>,
    },
}

impl Content {
    fn write_into(&self, fields: &mut Vec<(String, Nbt)>) {
        match self {
            Content::Text(text) => fields.push(("text".into(), Nbt::string(text))),
            Content::Translatable {
                key,
                fallback,
                with,
            } => {
                fields.push(("translate".into(), Nbt::string(key)));
                if let Some(fallback) = fallback {
                    fields.push(("fallback".into(), Nbt::string(fallback)));
                }
                if !with.is_empty() {
                    fields.push(("with".into(), list_of(with)));
                }
            }
            Content::Keybind(name) => fields.push(("keybind".into(), Nbt::string(name))),
            Content::Score { name, objective } => fields.push((
                "score".into(),
                Nbt::compound([
                    ("name", Nbt::string(name)),
                    ("objective", Nbt::string(objective)),
                ]),
            )),
            Content::Selector {
                selector,
                separator,
            } => {
                fields.push(("selector".into(), Nbt::string(selector)));
                if let Some(separator) = separator {
                    fields.push(("separator".into(), separator.to_nbt()));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// A text component: content, style, and a list of sibling components (`extra`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextComponent {
    pub content: Content,
    pub style: Style,
    pub extra: Vec<TextComponent>,
}

impl TextComponent {
    /// A bare component with the given content and an empty style / no siblings.
    pub fn new(content: Content) -> TextComponent {
        TextComponent {
            content,
            style: Style::default(),
            extra: Vec::new(),
        }
    }

    /// A plain literal component, `{text: s}`.
    pub fn text(s: impl Into<String>) -> TextComponent {
        TextComponent::new(Content::Text(s.into()))
    }

    /// A translatable component with the given key and (component) args.
    #[allow(dead_code)]
    pub fn translatable(key: impl Into<String>, with: Vec<TextComponent>) -> TextComponent {
        TextComponent::new(Content::Translatable {
            key: key.into(),
            fallback: None,
            with,
        })
    }

    /// A keybind component, `{keybind: id}`.
    #[allow(dead_code)]
    pub fn keybind(id: impl Into<String>) -> TextComponent {
        TextComponent::new(Content::Keybind(id.into()))
    }

    /// A scoreboard-value component, `{score: {name, objective}}`.
    #[allow(dead_code)]
    pub fn score(name: impl Into<String>, objective: impl Into<String>) -> TextComponent {
        TextComponent::new(Content::Score {
            name: name.into(),
            objective: objective.into(),
        })
    }

    /// An entity-selector component, `{selector: pattern}`.
    #[allow(dead_code)]
    pub fn selector(pattern: impl Into<String>) -> TextComponent {
        TextComponent::new(Content::Selector {
            selector: pattern.into(),
            separator: None,
        })
    }

    // -- chainable style/sibling setters ------------------------------------

    #[allow(dead_code)]
    pub fn with_color(mut self, color: Color) -> Self {
        self.style.color = Some(color);
        self
    }

    #[allow(dead_code)]
    pub fn with_bold(mut self, value: bool) -> Self {
        self.style.bold = Some(value);
        self
    }

    #[allow(dead_code)]
    pub fn with_italic(mut self, value: bool) -> Self {
        self.style.italic = Some(value);
        self
    }

    #[allow(dead_code)]
    pub fn with_underlined(mut self, value: bool) -> Self {
        self.style.underlined = Some(value);
        self
    }

    #[allow(dead_code)]
    pub fn with_strikethrough(mut self, value: bool) -> Self {
        self.style.strikethrough = Some(value);
        self
    }

    #[allow(dead_code)]
    pub fn with_obfuscated(mut self, value: bool) -> Self {
        self.style.obfuscated = Some(value);
        self
    }

    #[allow(dead_code)]
    pub fn with_click_event(mut self, event: ClickEvent) -> Self {
        self.style.click_event = Some(event);
        self
    }

    #[allow(dead_code)]
    pub fn with_hover_event(mut self, event: HoverEvent) -> Self {
        self.style.hover_event = Some(event);
        self
    }

    #[allow(dead_code)]
    pub fn with_insertion(mut self, value: impl Into<String>) -> Self {
        self.style.insertion = Some(value.into());
        self
    }

    #[allow(dead_code)]
    pub fn with_font(mut self, value: impl Into<String>) -> Self {
        self.style.font = Some(value.into());
        self
    }

    /// Append a sibling component to `extra`.
    #[allow(dead_code)]
    pub fn append(mut self, sibling: TextComponent) -> Self {
        self.extra.push(sibling);
        self
    }

    /// Encode to the network-NBT compound the client decodes: content fields,
    /// then style fields, then `extra` (when non-empty).
    pub fn to_nbt(&self) -> Nbt {
        let mut fields: Vec<(String, Nbt)> = Vec::new();
        self.content.write_into(&mut fields);
        if !self.style.is_empty() {
            self.style.write_into(&mut fields);
        }
        if !self.extra.is_empty() {
            fields.push(("extra".into(), list_of(&self.extra)));
        }
        Nbt::Compound(fields)
    }
}

/// Encode a slice of components as a homogeneous NBT list of compounds.
fn list_of(components: &[TextComponent]) -> Nbt {
    Nbt::List(components.iter().map(TextComponent::to_nbt).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field<'a>(nbt: &'a Nbt, key: &str) -> &'a Nbt {
        nbt.get(key)
            .unwrap_or_else(|| panic!("missing field {key} in {nbt:?}"))
    }

    #[test]
    fn literal_is_just_text() {
        let nbt = TextComponent::text("hi").to_nbt();
        assert_eq!(nbt.as_compound().unwrap().len(), 1);
        assert_eq!(field(&nbt, "text").as_str(), Some("hi"));
    }

    #[test]
    fn color_named_and_rgb() {
        assert_eq!(Color::named(NamedColor::Green).serialize(), "green");
        assert_eq!(Color::named(NamedColor::DarkBlue).serialize(), "dark_blue");
        assert_eq!(Color::rgb(0x55_FF55).serialize(), "#55FF55");
        assert_eq!(Color::parse("#0a0B0c"), Some(Color::Rgb(0x0A_0B0C)));
        assert_eq!(Color::parse("red"), Some(Color::Named(NamedColor::Red)));
        assert_eq!(Color::parse("not_a_color"), None); // unknown name rejected

        let nbt = TextComponent::text("x")
            .with_color(Color::named(NamedColor::Green))
            .to_nbt();
        assert_eq!(field(&nbt, "color").as_str(), Some("green"));
    }

    #[test]
    fn style_flags_are_bytes_with_snake_case_keys() {
        let nbt = TextComponent::text("x")
            .with_bold(true)
            .with_italic(false)
            .with_insertion("ins")
            .to_nbt();
        assert_eq!(field(&nbt, "bold"), &Nbt::Byte(1));
        assert_eq!(field(&nbt, "italic"), &Nbt::Byte(0));
        assert_eq!(field(&nbt, "insertion").as_str(), Some("ins"));
    }

    #[test]
    fn change_page_is_positive_and_uses_page_field() {
        let nbt = TextComponent::text("x")
            .with_click_event(ClickEvent::ChangePage(NonZeroU32::new(3).unwrap()))
            .to_nbt();
        let click = field(&nbt, "click_event");
        assert_eq!(field(click, "action").as_str(), Some("change_page"));
        assert_eq!(field(click, "page"), &Nbt::Int(3));
    }

    #[test]
    fn click_and_hover_events() {
        let nbt = TextComponent::text("x")
            .with_click_event(ClickEvent::CopyToClipboard("c".into()))
            .with_hover_event(HoverEvent::ShowText(Box::new(TextComponent::text("tip"))))
            .to_nbt();

        let click = field(&nbt, "click_event");
        assert_eq!(field(click, "action").as_str(), Some("copy_to_clipboard"));
        assert_eq!(field(click, "value").as_str(), Some("c"));

        let hover = field(&nbt, "hover_event");
        assert_eq!(field(hover, "action").as_str(), Some("show_text"));
        assert_eq!(field(field(hover, "value"), "text").as_str(), Some("tip"));
    }

    #[test]
    fn run_command_uses_command_field() {
        let nbt = TextComponent::text("x")
            .with_click_event(ClickEvent::RunCommand("/seed".into()))
            .to_nbt();
        let click = field(&nbt, "click_event");
        assert_eq!(field(click, "action").as_str(), Some("run_command"));
        assert_eq!(field(click, "command").as_str(), Some("/seed"));
    }

    #[test]
    fn translatable_with_and_extra_are_lists() {
        let nbt = TextComponent::translatable("k", vec![TextComponent::text("a")])
            .append(TextComponent::text("b"))
            .to_nbt();
        assert_eq!(field(&nbt, "translate").as_str(), Some("k"));
        match field(&nbt, "with") {
            Nbt::List(items) => assert_eq!(items.len(), 1),
            other => panic!("with not a list: {other:?}"),
        }
        match field(&nbt, "extra") {
            Nbt::List(items) => assert_eq!(items.len(), 1),
            other => panic!("extra not a list: {other:?}"),
        }
    }

    #[test]
    fn translatable_without_args_omits_with() {
        let nbt = TextComponent::translatable("k", vec![]).to_nbt();
        assert!(nbt.get("with").is_none());
    }

    #[test]
    fn score_and_selector_shapes() {
        let score = TextComponent::score("@p", "obj").to_nbt();
        let inner = field(&score, "score");
        assert_eq!(field(inner, "name").as_str(), Some("@p"));
        assert_eq!(field(inner, "objective").as_str(), Some("obj"));

        let sel = TextComponent::selector("@e").to_nbt();
        assert_eq!(field(&sel, "selector").as_str(), Some("@e"));
    }
}
