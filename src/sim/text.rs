//! Text component builders, producing the network-NBT shape the client decodes.
//!
//! A 26.2 chat component is an NBT compound: a content field (`text` for a
//! literal, `translate` (+ optional `with`) for a translatable) plus optional
//! sibling *style* keys (`color`, `clickEvent`, `hoverEvent`, `insertion`).
//! These mirror the decompiled `net.minecraft.network.chat` codecs — see
//! `Component`, `TranslatableContents`, `ClickEvent`, `HoverEvent` — without
//! copying any of their code.
//!
//! Translation args (`with`) are written as a homogeneous list of components.
//! Vanilla also permits raw primitive args (an int renders as its digits), but
//! a list mixing primitives and components is not a valid (single-type) NBT
//! list; wrapping every arg as a `{text:"…"}` component keeps the list
//! homogeneous and renders byte-for-byte identically, since the client
//! stringifies a numeric arg and a text component the same way.

use crate::protocol::nbt::Nbt;

/// A plain literal component: `{text: s}`.
pub fn text(s: impl Into<String>) -> Nbt {
    Nbt::Compound(vec![("text".to_string(), Nbt::String(s.into()))])
}

/// A translatable component: `{translate: key}` plus `{with: [args]}` when any
/// args are present. The client formats it through its own language file, so
/// the rendered text matches vanilla exactly for the same key and args.
pub fn translatable(key: &str, with: Vec<Nbt>) -> Nbt {
    let mut fields = vec![("translate".to_string(), Nbt::String(key.to_string()))];
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

/// Set the component's `color` (a named color like `"green"` or a `#rrggbb`).
pub fn colored(component: Nbt, color: &str) -> Nbt {
    with_field(component, "color", Nbt::String(color.to_string()))
}

/// Attach a copy-to-clipboard click event, mirroring `ClickEvent.CopyToClipboard`
/// (`{action: "copy_to_clipboard", value}`).
pub fn copy_to_clipboard(component: Nbt, value: &str) -> Nbt {
    let event = Nbt::Compound(vec![
        (
            "action".to_string(),
            Nbt::String("copy_to_clipboard".to_string()),
        ),
        ("value".to_string(), Nbt::String(value.to_string())),
    ]);
    with_field(component, "clickEvent", event)
}

/// Attach a show-text hover event, mirroring `HoverEvent.ShowText`
/// (`{action: "show_text", value: <component>}`).
pub fn hover_text(component: Nbt, value: Nbt) -> Nbt {
    let event = Nbt::Compound(vec![
        ("action".to_string(), Nbt::String("show_text".to_string())),
        ("value".to_string(), value),
    ]);
    with_field(component, "hoverEvent", event)
}

/// Set the shift-click `insertion` text.
pub fn insertion(component: Nbt, value: &str) -> Nbt {
    with_field(component, "insertion", Nbt::String(value.to_string()))
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
