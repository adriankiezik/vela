//! Text-component builders — a thin compatibility shim over the typed model in
//! [`crate::protocol::text`].
//!
//! These functions keep an incremental, `Nbt`-in/`Nbt`-out API (a base component
//! is built, then style/content keys are appended) that the command and chat
//! call sites rely on. The wire-shape knowledge — style key names, click/hover
//! event layout, color serialization — lives in `protocol::text`; this module
//! delegates to it so the bytes match the decompiled 26.2 `Component`/`Style`
//! codecs (notably the **snake_case** style keys `click_event` / `hover_event`).
//!
//! Translation args (`with`) and siblings are written as a homogeneous list of
//! component compounds, matching the wire (a primitive arg would be collapsed to
//! a bare string by vanilla, but a `{text:…}` compound decodes identically).

use crate::protocol::nbt::Nbt;
use crate::protocol::text::{ClickEvent, Color, TextComponent};

/// A plain literal component: `{text: s}`.
pub fn text(s: impl Into<String>) -> Nbt {
    TextComponent::text(s).to_nbt()
}

/// A translatable component: `{translate: key}` plus `{with: [args]}` when any
/// args are present. Args arrive already encoded as component compounds, so the
/// list is homogeneous. Mirrors `Content::Translatable` in `protocol::text`.
pub fn translatable(key: &str, with: Vec<Nbt>) -> Nbt {
    translatable_with_fallback(key, None, with)
}

/// Like [`translatable`] but with an optional `fallback` (rendered when the
/// client lacks the translation key). This keeps the same field order as
/// `protocol::text::Content::Translatable` — `translate`, then `fallback`, then
/// `with` — so the bytes match the codec. (The args are already-encoded `Nbt`,
/// so this can't take the typed `TextComponent` path directly; the field layout
/// is the single source of truth shared by both.)
pub fn translatable_with_fallback(key: &str, fallback: Option<&str>, with: Vec<Nbt>) -> Nbt {
    let mut fields = vec![("translate".to_string(), Nbt::string(key))];
    if let Some(fallback) = fallback {
        fields.push(("fallback".to_string(), Nbt::string(fallback)));
    }
    if !with.is_empty() {
        fields.push(("with".to_string(), Nbt::List(with)));
    }
    Nbt::Compound(fields)
}

/// Append a style/content key to a component compound. Panics in debug if the
/// component is not a compound — every builder here produces one.
fn with_field(component: Nbt, key: &str, value: Nbt) -> Nbt {
    match component {
        Nbt::Compound(mut fields) => {
            fields.push((key.to_string(), value));
            Nbt::Compound(fields)
        }
        other => {
            debug_assert!(false, "style applied to non-compound component: {other:?}");
            other
        }
    }
}

/// Set the component's `color` (one of the 16 named colors like `"green"`, or a
/// `#rrggbb` value), serialized through `protocol::text::Color`. An unrecognized
/// color is rejected (debug-asserted) and leaves the component unchanged rather
/// than emitting a name the client would refuse.
pub fn colored(component: Nbt, color: &str) -> Nbt {
    match Color::parse(color) {
        Some(c) => with_field(component, "color", Nbt::string(c.serialize())),
        None => {
            debug_assert!(false, "invalid text color: {color}");
            component
        }
    }
}

/// Attach a copy-to-clipboard click event, delegating to
/// `ClickEvent::CopyToClipboard` (`{action: "copy_to_clipboard", value}`).
pub fn copy_to_clipboard(component: Nbt, value: &str) -> Nbt {
    with_field(
        component,
        "click_event",
        ClickEvent::CopyToClipboard(value.to_string()).to_nbt(),
    )
}

/// Attach a show-text hover event (`{action: "show_text", value: <component>}`,
/// mirroring `HoverEvent::ShowText`). `value` is an already-encoded component.
pub fn hover_text(component: Nbt, value: Nbt) -> Nbt {
    let event = Nbt::compound([("action", Nbt::string("show_text")), ("value", value)]);
    with_field(component, "hover_event", event)
}

/// Set the shift-click `insertion` text.
pub fn insertion(component: Nbt, value: &str) -> Nbt {
    with_field(component, "insertion", Nbt::string(value))
}

/// Wrap a component in square brackets, mirroring `ComponentUtils.wrapInSquareBrackets`
/// (`{translate: "chat.square_brackets", with: [inner]}`).
pub fn square_brackets(inner: Nbt) -> Nbt {
    translatable("chat.square_brackets", vec![inner])
}

/// `ComponentUtils.copyOnClickText`: a green, bracketed value that copies itself
/// to the clipboard on click and shows the standard "click to copy" tooltip.
pub fn copy_on_click(value: &str) -> Nbt {
    let inner = insertion(
        hover_text(
            copy_to_clipboard(colored(text(value), "green"), value),
            translatable("chat.copy.click", vec![]),
        ),
        value,
    );
    square_brackets(inner)
}
