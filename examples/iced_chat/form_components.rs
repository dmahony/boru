//! Reusable Boru form primitives (UI-RESTYLE-03).
//!
//! Styled building blocks used by the creation dialogs (Create Group Chat,
//! Create Public Room, Create Tunnel) and any future form surface. Every
//! component is a pure function or builder struct that composes the shared
//! design tokens from `design_tokens` and reuses existing primitives from
//! `ui_components` (buttons, text-input styling) instead of maintaining
//! parallel style definitions.
//!
//! ## Component catalogue
//!
//! | Component               | Builder / fn                  | Notes                              |
//! |-------------------------|-------------------------------|------------------------------------|
//! | Field label             | `form_label(…)`               | label above a control              |
//! | Helper text             | `helper_text(…)`              | muted support text below a control |
//! | Error text              | `error_text(…)`               | danger support text below a control|
//! | Form section            | `FormSection::new(…)`         | titled group with optional helper  |
//! | Labelled text input     | `TextInput::new(…)`           | label + input + helper/error       |
//! | Multiline text area     | `TextArea::new(…)`            | label + editor + helper/error      |
//! | Select / dropdown       | `Select::new(…)`              | pick-list wrapper                  |
//! | Searchable select       | `SearchableSelect::new(…)`    | combo-box wrapper                  |
//! | Checkbox                | `checkbox_field(…)`           | styled checkbox with label         |
//! | Toggle / switch         | `toggle_field(…)`             | styled toggler with label          |
//! | Selectable peer row     | `SelectablePeerRow::new(…)`   | avatar + label + checkbox row      |
//! | Peer list panel         | `peer_list(…)`                | bordered scrollable list           |
//! | Selectable peer list    | `SelectablePeerList::new(…)`  | search + chips + list + summary    |
//! | Removable chip          | `remove_chip(…)`              | pill with an × remove button       |
//! | Selection summary       | `selection_summary(…)`        | "N participant(s) selected"        |
//! | Dialog footer           | `DialogFooter::new(…)`        | Cancel + primary action row        |
//! | Destructive button      | `destructive_button(…)`       | danger-filled action               |
//!
//! Buttons: reuse `ui_components::primary_button` / `secondary_button` /
//! `ghost_icon_button` directly — those are the canonical Boru buttons and are
//! not re-exported here to avoid parallel versions.
//!
//! ## Text area state
//!
//! [`TextArea`] uses iced's `text_editor` widget, so the caller owns a
//! [`text_editor::Content`] value:
//!
//! ```rust,ignore
//! // state:
//! description: text_editor::Content,
//!
//! // init:
//! description: text_editor::Content::with_text(""),
//!
//! // update:
//! AppMessage::DescriptionEdited(action) => self.description.perform(action),
//!
//! // view:
//! TextArea::new("Description", &self.description, AppMessage::DescriptionEdited)
//!     .placeholder("Optional description…")
//!     .build(),
//!
//! // submit:
//! self.description.text()
//! ```

use iced::widget::{
    button, checkbox, combo_box, container, pick_list, radio, text, text_editor, toggler,
    Column, Row, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::AppMessage;
use crate::design_tokens;
use crate::fonts::TypeRole;
use crate::ui_components;

// ═══════════════════════════════════════════════════════════════════════
// 1. FORM TEXT PRIMITIVES — label / helper / error
// ═══════════════════════════════════════════════════════════════════════

/// A form field label rendered above a control.
///
/// Uses the `ButtonLabel` typography role (IBM Plex Sans SemiBold, 14 px)
/// with secondary text colour so it reads as a label, not body copy.
/// (FONTS-04/11: form labels are IBM Plex Sans Medium-or-SemiBold.)
pub fn form_label(label: &str) -> Element<'static, AppMessage> {
    text(label.to_string())
        .font(TypeRole::ButtonLabel.font())
        .size(TypeRole::ButtonLabel.size_px())
        .style(|t| text::Style {
            color: Some(design_tokens::text_secondary(t)),
        })
        .into()
}

/// A muted helper line rendered below a control to explain what goes there.
pub fn helper_text(helper: &str) -> Element<'static, AppMessage> {
    text(helper.to_string())
        .font(TypeRole::SupportingText.font())
        .size(TypeRole::SupportingText.size_px())
        .style(|t| text::Style {
            color: Some(design_tokens::text_muted(t)),
        })
        .into()
}

/// A danger-coloured error line rendered below a control.
///
/// Kept as plain text (no harsh background) so the error state is visible
/// without shouting — matching the shared form-styling guidance.
pub fn error_text(error: &str) -> Element<'static, AppMessage> {
    text(error.to_string())
        .font(TypeRole::SupportingText.font())
        .size(TypeRole::SupportingText.size_px())
        .style(|t| text::Style {
            color: Some(design_tokens::color_danger(t)),
        })
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 2. FORM SECTION — titled group with optional helper
// ═══════════════════════════════════════════════════════════════════════

/// A titled form section: a compact heading, an optional helper line, and the
/// section's fields.
pub struct FormSection<'a> {
    title: String,
    helper: Option<String>,
    children: Vec<Element<'a, AppMessage>>,
}

impl<'a> FormSection<'a> {
    /// Start a section with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            helper: None,
            children: Vec::new(),
        }
    }

    /// Add an optional helper line below the title.
    pub fn helper(mut self, helper: impl Into<String>) -> Self {
        self.helper = Some(helper.into());
        self
    }

    /// Append a child element (field, row, panel…).
    pub fn push(mut self, child: Element<'a, AppMessage>) -> Self {
        self.children.push(child);
        self
    }

    /// Build the section element.
    pub fn build(self) -> Element<'a, AppMessage> {
        let mut col = Column::new()
            .push(
                text(self.title)
                    .font(TypeRole::ButtonLabel.font())
                    .size(TypeRole::ButtonLabel.size_px())
                    .style(|t| text::Style {
                        color: Some(design_tokens::text_primary(t)),
                    }),
            )
            .spacing(design_tokens::SPACE_8)
            .width(Length::Fill);

        if let Some(helper) = self.helper {
            col = col.push(helper_text(&helper));
        }

        for child in self.children {
            col = col.push(child);
        }

        col.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 3. LABELLED TEXT INPUT
// ═══════════════════════════════════════════════════════════════════════

/// A labelled text input with optional helper and error text.
///
/// The input widget itself is `ui_components::text_input_field` — the shared
/// Boru text input — wrapped with the label/helper/error scaffolding.
pub struct TextInput<'a> {
    label: String,
    placeholder: String,
    value: String,
    on_input: Box<dyn Fn(String) -> AppMessage + 'a>,
    helper: Option<String>,
    error: Option<String>,
    /// Optional focus [`iced::widget::text_input::Id`] (static str) so the
    /// dialog can auto-focus this field on open and Tab can reach it.
    id: Option<&'static str>,
    /// Optional Enter-to-submit message for the dialog's primary field.
    on_submit: Option<AppMessage>,
}

impl<'a> TextInput<'a> {
    /// Start a labelled input bound to `value` and `on_input`.
    pub fn new(
        label: impl Into<String>,
        placeholder: &str,
        value: &str,
        on_input: impl Fn(String) -> AppMessage + 'a,
    ) -> Self {
        Self {
            label: label.into(),
            placeholder: placeholder.to_string(),
            value: value.to_string(),
            on_input: Box::new(on_input),
            helper: None,
            error: None,
            id: None,
            on_submit: None,
        }
    }

    /// Add an optional helper line below the input.
    pub fn helper(mut self, helper: impl Into<String>) -> Self {
        self.helper = Some(helper.into());
        self
    }

    /// Mark the field as errored and show the error line below the input.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Give the field a stable focus id (static str) so the dialog can
    /// auto-focus it on open and Tab order is deterministic.
    pub fn id(mut self, id: &'static str) -> Self {
        self.id = Some(id);
        self
    }

    /// Submit the dialog form when Enter is pressed in this field.
    pub fn on_submit(mut self, message: AppMessage) -> Self {
        self.on_submit = Some(message);
        self
    }

    /// Build the labelled field.
    pub fn build(self) -> Element<'a, AppMessage> {
        let has_error = self.error.is_some();

        let input = ui_components::text_input_field_opts(
            &self.placeholder,
            &self.value,
            self.on_input,
            has_error,
            self.id,
            self.on_submit,
        );

        let mut col = Column::new()
            .push(form_label(&self.label))
            .push(input)
            .spacing(design_tokens::SPACE_4)
            .width(Length::Fill);

        if let Some(helper) = self.helper {
            col = col.push(helper_text(&helper));
        }
        if let Some(error) = self.error {
            col = col.push(error_text(&error));
        }

        col.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 4. MULTILINE TEXT AREA
// ═══════════════════════════════════════════════════════════════════════

/// Style for a multiline text area — mirrors the shared text-input styling
/// (input background, muted border, focus ring, medium radius) so the two
/// field types look consistent.
fn text_editor_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let (border_color, border_width) = match status {
        text_editor::Status::Focused { .. } => (
            design_tokens::color_focus(theme),
            design_tokens::FOCUS_WIDTH,
        ),
        _ => (design_tokens::border_muted(theme), design_tokens::BORDER_WIDTH),
    };
    text_editor::Style {
        background: Background::Color(design_tokens::bg_input(theme)),
        border: Border {
            color: border_color,
            width: border_width,
            radius: design_tokens::RADIUS_MD.into(),
        },
        placeholder: design_tokens::text_muted(theme),
        value: design_tokens::text_primary(theme),
        selection: design_tokens::primary_soft(theme),
    }
}

/// Style for a multiline text area in error state — danger border.
fn text_editor_error_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let mut base = text_editor_style(theme, status);
    base.border = Border {
        color: design_tokens::color_danger(theme),
        width: design_tokens::BORDER_WIDTH,
        radius: design_tokens::RADIUS_MD.into(),
    };
    base
}

/// A labelled multiline text area with optional helper and error text.
///
/// See the module docs for the `text_editor::Content` state pattern.
pub struct TextArea<'a> {
    label: String,
    content: &'a text_editor::Content,
    on_action: Box<dyn Fn(text_editor::Action) -> AppMessage + 'a>,
    placeholder: String,
    helper: Option<String>,
    error: Option<String>,
    min_height: f32,
}

impl<'a> TextArea<'a> {
    /// Start a labelled text area bound to the caller-owned `content` and the
    /// `on_action` message constructor.
    pub fn new(
        label: impl Into<String>,
        content: &'a text_editor::Content,
        on_action: impl Fn(text_editor::Action) -> AppMessage + 'a,
    ) -> Self {
        Self {
            label: label.into(),
            content,
            on_action: Box::new(on_action),
            placeholder: String::new(),
            helper: None,
            error: None,
            min_height: 96.0,
        }
    }

    /// Set the placeholder text shown when the editor is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Add an optional helper line below the editor.
    pub fn helper(mut self, helper: impl Into<String>) -> Self {
        self.helper = Some(helper.into());
        self
    }

    /// Mark the field as errored and show the error line below the editor.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Override the default minimum height (96 px).
    pub fn min_height(mut self, px: f32) -> Self {
        self.min_height = px;
        self
    }

    /// Build the labelled text area.
    pub fn build(self) -> Element<'a, AppMessage> {
        let has_error = self.error.is_some();

        let mut editor = text_editor(self.content)
            .on_action(self.on_action)
            .placeholder(self.placeholder)
            .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
            .height(Length::Fixed(self.min_height))
            // FONTS Task 11: input text is IBM Plex Sans Regular (Body role).
            .font(TypeRole::Body.font());

        editor = if has_error {
            editor.style(text_editor_error_style)
        } else {
            editor.style(text_editor_style)
        };

        let mut col = Column::new()
            .push(form_label(&self.label))
            .push(editor)
            .spacing(design_tokens::SPACE_4)
            .width(Length::Fill);

        if let Some(helper) = self.helper {
            col = col.push(helper_text(&helper));
        }
        if let Some(error) = self.error {
            col = col.push(error_text(&error));
        }

        col.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 5. SELECT / DROPDOWN
// ═══════════════════════════════════════════════════════════════════════

/// Style for a pick-list — mirrors the shared text-input styling.
fn pick_list_style(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let border_color = match status {
        pick_list::Status::Hovered | pick_list::Status::Opened { .. } => {
            design_tokens::border_strong(theme)
        }
        _ => design_tokens::border_muted(theme),
    };
    pick_list::Style {
        text_color: design_tokens::text_primary(theme),
        placeholder_color: design_tokens::text_muted(theme),
        handle_color: design_tokens::text_secondary(theme),
        background: Background::Color(design_tokens::bg_input(theme)),
        border: Border {
            color: border_color,
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_MD.into(),
        },
    }
}

/// Menu (popover) style for selects — surface background, soft border,
/// elevated shadow, primary highlight for the selected option.
fn select_menu_style(theme: &Theme) -> iced::widget::overlay::menu::Style {
    iced::widget::overlay::menu::Style {
        background: Background::Color(design_tokens::surface(theme)),
        border: Border {
            color: design_tokens::border_muted(theme),
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_MD.into(),
        },
        text_color: design_tokens::text_primary(theme),
        selected_text_color: design_tokens::primary(theme),
        selected_background: Background::Color(design_tokens::primary_soft(theme)),
        shadow: design_tokens::shadow_elevated(theme),
    }
}

/// A labelled dropdown built on iced's `pick_list`.
pub struct Select<'a, T> {
    label: String,
    options: Vec<T>,
    selected: Option<T>,
    on_selected: Box<dyn Fn(T) -> AppMessage + 'a>,
    placeholder: String,
    helper: Option<String>,
    error: Option<String>,
}

impl<'a, T> Select<'a, T>
where
    T: ToString + PartialEq + Clone + 'a,
{
    /// Start a labelled select with the given options, current selection, and
    /// the message constructor called with the newly selected option.
    pub fn new(
        label: impl Into<String>,
        options: Vec<T>,
        selected: Option<T>,
        on_selected: impl Fn(T) -> AppMessage + 'a,
    ) -> Self {
        Self {
            label: label.into(),
            options,
            selected,
            on_selected: Box::new(on_selected),
            placeholder: String::new(),
            helper: None,
            error: None,
        }
    }

    /// Set the placeholder shown when nothing is selected.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Add an optional helper line below the select.
    pub fn helper(mut self, helper: impl Into<String>) -> Self {
        self.helper = Some(helper.into());
        self
    }

    /// Mark the field as errored and show the error line below the select.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Build the labelled select.
    pub fn build(self) -> Element<'a, AppMessage> {
        let mut list = pick_list(self.options, self.selected, self.on_selected)
            .placeholder(self.placeholder)
            .width(Length::Fill)
            .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
            .text_size(TypeRole::Body.size_px())
            // FONTS Task 11: select text is IBM Plex Sans Regular (Body role).
            .font(TypeRole::Body.font())
            .style(pick_list_style)
            .menu_style(select_menu_style);

        if self.error.is_some() {
            list = list.style(|t, s| pick_list::Style {
                border: Border {
                    color: design_tokens::color_danger(t),
                    width: design_tokens::BORDER_WIDTH,
                    radius: design_tokens::RADIUS_MD.into(),
                },
                ..pick_list_style(t, s)
            });
        }

        let mut col = Column::new()
            .push(form_label(&self.label))
            .push(list)
            .spacing(design_tokens::SPACE_4)
            .width(Length::Fill);

        if let Some(helper) = self.helper {
            col = col.push(helper_text(&helper));
        }
        if let Some(error) = self.error {
            col = col.push(error_text(&error));
        }

        col.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 6. SEARCHABLE SELECT (COMBO BOX)
// ═══════════════════════════════════════════════════════════════════════

/// A labelled searchable select built on iced's `combo_box`.
///
/// The caller owns a [`combo_box::State<T>`] (exactly like the existing
/// tunnel expiry picker in `app.rs`), which stores the option list.
pub struct SearchableSelect<'a, T> {
    label: String,
    state: &'a combo_box::State<T>,
    placeholder: String,
    selected: Option<&'a T>,
    on_selected: Box<dyn Fn(T) -> AppMessage + 'static>,
    helper: Option<String>,
}

impl<'a, T> SearchableSelect<'a, T>
where
    T: std::fmt::Display + Clone + 'static,
{
    /// Start a labelled searchable select bound to the caller-owned `state`.
    pub fn new(
        label: impl Into<String>,
        state: &'a combo_box::State<T>,
        placeholder: &str,
        selected: Option<&'a T>,
        on_selected: impl Fn(T) -> AppMessage + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            state,
            placeholder: placeholder.to_string(),
            selected,
            on_selected: Box::new(on_selected),
            helper: None,
        }
    }

    /// Add an optional helper line below the select.
    pub fn helper(mut self, helper: impl Into<String>) -> Self {
        self.helper = Some(helper.into());
        self
    }

    /// Build the labelled searchable select.
    pub fn build(self) -> Element<'a, AppMessage> {
        let combo = combo_box::ComboBox::new(
            self.state,
            &self.placeholder,
            self.selected,
            self.on_selected,
        )
        .width(Length::Fill)
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
        // FONTS Task 11: combo-box input text is IBM Plex Sans Regular
        // (Body role), matching the other form inputs.
        .font(TypeRole::Body.font())
        .input_style(ui_components::text_input_style)
        .menu_style(select_menu_style);

        let mut col = Column::new()
            .push(form_label(&self.label))
            .push(combo)
            .spacing(design_tokens::SPACE_4)
            .width(Length::Fill);

        if let Some(helper) = self.helper {
            col = col.push(helper_text(&helper));
        }

        col.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 7. CHECKBOX & TOGGLE
// ═══════════════════════════════════════════════════════════════════════

/// Style for a Boru checkbox — primary green fill + white check when checked,
/// soft surface with muted border when not.
fn checkbox_style(theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let is_checked = match status {
        checkbox::Status::Active { is_checked }
        | checkbox::Status::Hovered { is_checked }
        | checkbox::Status::Disabled { is_checked } => is_checked,
    };

    let border_color = if is_checked {
        design_tokens::primary(theme)
    } else {
        match status {
            checkbox::Status::Hovered { .. } => design_tokens::border_strong(theme),
            _ => design_tokens::border_muted(theme),
        }
    };

    checkbox::Style {
        background: Background::Color(if is_checked {
            design_tokens::primary(theme)
        } else {
            design_tokens::surface(theme)
        }),
        icon_color: Color::WHITE,
        border: Border {
            color: border_color,
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_SM.into(),
        },
        text_color: Some(design_tokens::text_primary(theme)),
    }
}

/// A styled checkbox with an optional helper line.
pub fn checkbox_field<'a>(
    label: impl Into<String>,
    is_checked: bool,
    on_toggle: impl Fn(bool) -> AppMessage + 'a,
    helper: Option<String>,
) -> Element<'a, AppMessage> {
    let cb = checkbox(is_checked)
        .label(label.into())
        .on_toggle(on_toggle)
        .text_size(TypeRole::Body.size_px())
        // FONTS Task 11: checkbox label is IBM Plex Sans Regular (Body role).
        .font(TypeRole::Body.font())
        .style(checkbox_style);

    match helper {
        Some(h) => Column::new()
            .push(cb)
            .push(helper_text(&h))
            .spacing(design_tokens::SPACE_2)
            .width(Length::Fill)
            .into(),
        None => cb.into(),
    }
}

/// Style for a Boru radio button — primary ring + white dot when selected,
/// soft surface with muted border when not.
fn radio_style(theme: &Theme, status: radio::Status) -> radio::Style {
    let is_selected = match status {
        radio::Status::Active { is_selected } | radio::Status::Hovered { is_selected } => {
            is_selected
        }
    };

    radio::Style {
        background: Background::Color(if is_selected {
            design_tokens::primary(theme)
        } else {
            design_tokens::surface(theme)
        }),
        dot_color: Color::WHITE,
        border_width: design_tokens::BORDER_WIDTH,
        border_color: if is_selected {
            design_tokens::primary(theme)
        } else {
            design_tokens::border_muted(theme)
        },
        text_color: Some(design_tokens::text_primary(theme)),
    }
}

/// A styled radio button with an optional helper line.
pub fn radio_field<'a, V>(
    label: &'a str,
    value: V,
    selected: Option<V>,
    on_selected: impl Fn(V) -> AppMessage + 'a,
    helper: Option<&'a str>,
) -> Element<'a, AppMessage>
where
    V: Copy + Eq + 'a,
{
    let rb = radio(label, value, selected, on_selected)
        .text_size(TypeRole::Body.size_px())
        // FONTS Task 11: radio label is IBM Plex Sans Regular (Body role).
        .font(TypeRole::Body.font())
        .style(radio_style);

    match helper {
        Some(h) => Column::new()
            .push(rb)
            .push(helper_text(h))
            .spacing(design_tokens::SPACE_2)
            .width(Length::Fill)
            .into(),
        None => rb.into(),
    }
}

/// Style for a Boru toggle — primary green track + white knob when on, muted
/// track when off. Shared by the sender screen-share audio switch
/// (BORU-SSUI-06) so every toggle in the app looks identical.
pub(crate) fn toggler_style(theme: &Theme, status: toggler::Status) -> toggler::Style {
    let is_toggled = match status {
        toggler::Status::Active { is_toggled }
        | toggler::Status::Hovered { is_toggled }
        | toggler::Status::Disabled { is_toggled } => is_toggled,
    };

    toggler::Style {
        background: Background::Color(if is_toggled {
            design_tokens::primary(theme)
        } else {
            design_tokens::border_muted(theme)
        }),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Background::Color(Color::WHITE),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(design_tokens::text_primary(theme)),
        border_radius: Some(design_tokens::SPACE_12.into()),
        padding_ratio: 0.5,
    }
}

/// A styled toggle/switch with an optional helper line.
pub fn toggle_field<'a>(
    label: &'a str,
    is_toggled: bool,
    on_toggle: impl Fn(bool) -> AppMessage + 'a,
    helper: Option<&'a str>,
) -> Element<'a, AppMessage> {
    let tg = toggler(is_toggled)
        .label(label)
        .on_toggle(on_toggle)
        // FONTS Task 11: toggle label is IBM Plex Sans Regular (Body role).
        .text_size(TypeRole::Body.size_px())
        .font(TypeRole::Body.font())
        .style(toggler_style);

    match helper {
        Some(h) => Column::new()
            .push(tg)
            .push(helper_text(h))
            .spacing(design_tokens::SPACE_2)
            .width(Length::Fill)
            .into(),
        None => tg.into(),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 8. SELECTABLE PEER ROW + PEER LIST PANEL
// ═══════════════════════════════════════════════════════════════════════

/// A single selectable peer row: optional avatar, display label, optional
/// secondary line, and a trailing checkbox that dispatches `on_toggle`.
///
/// This replaces the friend-picker loop duplicated in the group-chat, tunnel,
/// and invite-member dialogs (UI-RESTYLE-01 finding 4).
pub struct SelectablePeerRow<'a> {
    label: String,
    secondary: Option<String>,
    avatar: Option<Element<'a, AppMessage>>,
    selected: bool,
    on_toggle: Option<AppMessage>,
}

impl<'a> SelectablePeerRow<'a> {
    /// Start a peer row with the given display label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            secondary: None,
            avatar: None,
            selected: false,
            on_toggle: None,
        }
    }

    /// Add a secondary line (peer id, status…).
    pub fn secondary(mut self, text: impl Into<String>) -> Self {
        self.secondary = Some(text.into());
        self
    }

    /// Add an avatar element (e.g. `ui_components::Avatar`).
    pub fn avatar(mut self, avatar: Element<'a, AppMessage>) -> Self {
        self.avatar = Some(avatar);
        self
    }

    /// Mark the row as selected (checkbox checked + selected surface).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Dispatch `msg` when the checkbox is toggled. When `None`, the checkbox
    /// is rendered disabled.
    pub fn on_toggle(mut self, msg: AppMessage) -> Self {
        self.on_toggle = Some(msg);
        self
    }

    /// Build the row element.
    pub fn build(self, theme: &Theme) -> Element<'a, AppMessage> {
        let mut row = Row::new()
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        if let Some(avatar) = self.avatar {
            row = row.push(avatar);
        }

        let mut text_col = Column::new()
            .push(
                text(self.label)
                    .font(TypeRole::Body.font())
                    .size(TypeRole::Body.size_px())
                    .style(|t| text::Style {
                        color: Some(design_tokens::text_primary(t)),
                    }),
            )
            .spacing(design_tokens::SPACE_2)
            .width(Length::Fill);

        if let Some(secondary) = self.secondary {
            text_col = text_col.push(
                text(secondary)
                    .font(TypeRole::SupportingText.font())
                    .size(TypeRole::SupportingText.size_px())
                    .style(|t| text::Style {
                        color: Some(design_tokens::text_muted(t)),
                    }),
            );
        }

        row = row.push(text_col);

        if let Some(msg) = self.on_toggle {
            row = row.push(
                checkbox(self.selected)
                    .on_toggle(move |_| msg.clone())
                    .style(checkbox_style),
            );
        }

        let selected = self.selected;
        let bg = if selected {
            design_tokens::surface_selected(theme)
        } else {
            Color::TRANSPARENT
        };
        container(row)
            .padding([design_tokens::SPACE_6, design_tokens::SPACE_8])
            .width(Length::Fill)
            .style(move |_t| container::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }
}

/// A bordered, scrollable panel that renders peer rows (or any rows).
///
/// Replaces the per-dialog `scrollable(…).height(Fixed(200|250))` inner
/// container style duplicated across the creation dialogs.
pub fn peer_list<'a>(
    rows: Vec<Element<'a, AppMessage>>,
    max_height: f32,
    empty_text: Option<String>,
) -> Element<'a, AppMessage> {
    let inner: Element<'a, AppMessage> = if rows.is_empty() {
        match empty_text {
            Some(empty) => container(
                text(empty)
                    .font(TypeRole::SupportingText.font())
                    .size(TypeRole::SupportingText.size_px())
                    .style(|t| text::Style {
                        color: Some(design_tokens::text_muted(t)),
                    }),
            )
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding(design_tokens::SPACE_12)
            .into(),
            None => Space::new().width(Length::Fill).height(Length::Shrink).into(),
        }
    } else {
        crate::ui_components::gutter_scrollable(
            Column::with_children(rows)
                .spacing(design_tokens::SPACE_4)
                .width(Length::Fill),
        )
        .height(Length::Fixed(max_height))
        .into()
    };

    container(inner)
        .width(Length::Fill)
        .padding(design_tokens::SPACE_8)
        .style(|t| container::Style {
            background: Some(Background::Color(design_tokens::surface_hover(t))),
            border: Border {
                color: design_tokens::border_muted(t),
                width: design_tokens::BORDER_WIDTH,
                radius: design_tokens::RADIUS_MD.into(),
            },
            ..Default::default()
        })
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 9. CHIPS / TAGS FOR SELECTED PEERS
// ═══════════════════════════════════════════════════════════════════════

/// A pill/tag chip showing a selected peer, with an optional × remove button.
///
/// Accepts an owned label (or any `Into<String>`) so callers can pass
/// dynamically computed display names without lifetime gymnastics.
pub fn remove_chip(
    label: impl Into<String>,
    on_remove: Option<AppMessage>,
) -> Element<'static, AppMessage> {
    let label = label.into();
    let mut chip = Row::new()
        .push(
            text(label.clone())
                .font(TypeRole::Metadata.font())
                .size(TypeRole::Metadata.size_px())
                .style(|t| text::Style {
                    color: Some(design_tokens::text_primary(t)),
                }),
        )
        .spacing(design_tokens::SPACE_4)
        .align_y(Alignment::Center);

    if let Some(msg) = on_remove {
        chip = chip.push(
            button(
                text("\u{2715}")
                    .size(crate::theme::BoruTheme::default().typography.badge)
                    .style(|t| text::Style {
                        color: Some(design_tokens::text_muted(t)),
                    }),
            )
            .on_press(msg)
            .padding(0)
            .style(|t, status| button::Style {
                text_color: match status {
                    button::Status::Hovered => design_tokens::color_danger(t),
                    _ => design_tokens::text_muted(t),
                },
                ..Default::default()
            }),
        );
    }

    container(chip)
        .padding([design_tokens::SPACE_4, design_tokens::SPACE_8])
        .height(Length::Fixed(design_tokens::CHIP_HEIGHT))
        .align_y(Alignment::Center)
        .style(|t| container::Style {
            background: Some(Background::Color(design_tokens::primary_soft(t))),
            border: Border {
                color: design_tokens::border_muted(t),
                width: design_tokens::BORDER_WIDTH,
                radius: design_tokens::SPACE_12.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A compact "N participant(s) selected" summary line.
pub fn selection_summary(count: usize, noun: &str) -> Element<'static, AppMessage> {
    let label = if count == 1 {
        format!("1 {noun} selected")
    } else {
        format!("{count} {noun}s selected")
    };
    helper_text(&label)
}

// ═══════════════════════════════════════════════════════════════════════
// 9b. SELECTABLE PEER LIST — search + chips + list + summary
// ═══════════════════════════════════════════════════════════════════════

/// A complete selectable-peer picker: optional search box, selected chips,
/// the scrollable peer list, and an optional selection summary.
///
/// Composes the lower-level `text_input_field`, `remove_chip`, `peer_list`
/// and `selection_summary` primitives so the creation dialogs don't rebuild
/// the same scaffolding. Callers build the peer rows themselves (with
/// [`SelectablePeerRow`]) so avatars/presence/secondary text stay flexible.
pub struct SelectablePeerList<'a> {
    rows: Vec<Element<'a, AppMessage>>,
    max_height: f32,
    empty_text: Option<String>,
    search: Option<(String, &'a str, Box<dyn Fn(String) -> AppMessage + 'a>)>,
    chips: Vec<Element<'a, AppMessage>>,
    summary: Option<(usize, String)>,
}

impl<'a> SelectablePeerList<'a> {
    /// Start a peer picker with the given pre-built rows, list height, and
    /// optional empty-state text.
    pub fn new(
        rows: Vec<Element<'a, AppMessage>>,
        max_height: f32,
        empty_text: Option<String>,
    ) -> Self {
        Self {
            rows,
            max_height,
            empty_text,
            search: None,
            chips: Vec::new(),
            summary: None,
        }
    }

    /// Add a search/filter field above the list.
    pub fn search(
        mut self,
        placeholder: impl Into<String>,
        value: &'a str,
        on_input: impl Fn(String) -> AppMessage + 'a,
    ) -> Self {
        self.search = Some((placeholder.into(), value, Box::new(on_input)));
        self
    }

    /// Add removable chips for the currently selected peers.
    pub fn chips(mut self, chips: Vec<Element<'a, AppMessage>>) -> Self {
        self.chips = chips;
        self
    }

    /// Add an "N noun(s) selected" summary line below the list.
    pub fn summary(mut self, count: usize, noun: impl Into<String>) -> Self {
        self.summary = Some((count, noun.into()));
        self
    }

    /// Build the picker column: search field, chips row, peer list, summary.
    pub fn build(mut self) -> Element<'a, AppMessage> {
        let mut col = Column::new().spacing(design_tokens::SPACE_8);

        if let Some((placeholder, value, on_input)) = self.search {
            col = col.push(ui_components::text_input_field(
                &placeholder,
                value,
                on_input,
                false,
            ));
        }

        if !self.chips.is_empty() {
            let mut chip_row = Row::new().spacing(design_tokens::SPACE_4);
            for chip in self.chips {
                chip_row = chip_row.push(chip);
            }
            col = col.push(chip_row);
        }

        col = col.push(peer_list(
            self.rows,
            self.max_height,
            self.empty_text.take(),
        ));

        if let Some((count, noun)) = self.summary {
            col = col.push(selection_summary(count, &noun));
        }

        col.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 10. DIALOG FOOTER
// ═══════════════════════════════════════════════════════════════════════

/// The standard dialog action row: optional secondary action (Cancel) on the
/// left of the primary action (Create / Save / Share), right-aligned.
///
/// Reuses the canonical `secondary_button` / `primary_button` components.
pub struct DialogFooter<'a> {
    cancel: Option<(&'a str, AppMessage)>,
    confirm: Option<(&'a str, AppMessage)>,
    confirm_disabled: bool,
    extra: Vec<Element<'a, AppMessage>>,
}

impl<'a> DialogFooter<'a> {
    /// Start an empty footer.
    pub fn new() -> Self {
        Self {
            cancel: None,
            confirm: None,
            confirm_disabled: false,
            extra: Vec::new(),
        }
    }

    /// Add the secondary (cancel) action.
    pub fn cancel(mut self, label: &'a str, msg: AppMessage) -> Self {
        self.cancel = Some((label, msg));
        self
    }

    /// Add the primary (confirm) action.
    pub fn confirm(mut self, label: &'a str, msg: AppMessage) -> Self {
        self.confirm = Some((label, msg));
        self
    }

    /// Disable the primary action (e.g. while a required field is empty).
    pub fn confirm_disabled(mut self, disabled: bool) -> Self {
        self.confirm_disabled = disabled;
        self
    }

    /// Append an extra element (e.g. a destructive action on the left).
    pub fn push(mut self, element: Element<'a, AppMessage>) -> Self {
        self.extra.push(element);
        self
    }

    /// Build the footer row.
    pub fn build(self) -> Element<'a, AppMessage> {
        let mut row = Row::new()
            .spacing(design_tokens::SPACE_12)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        for element in self.extra {
            row = row.push(element);
        }

        row = row.push(Space::new().width(Length::Fill).height(Length::Shrink));

        if let Some((label, msg)) = self.cancel {
            row = row.push(ui_components::secondary_button(label, Some(msg), false));
        }
        if let Some((label, msg)) = self.confirm {
            row = row.push(ui_components::primary_button(label, Some(msg), self.confirm_disabled));
        }

        row.into()
    }
}

impl<'a> Default for DialogFooter<'a> {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 11. DESTRUCTIVE BUTTON
// ═══════════════════════════════════════════════════════════════════════

/// Filled destructive button style — danger background, white text.
pub fn button_destructive_style(theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => {
            let mut c = design_tokens::color_danger(theme);
            c.r *= 0.9;
            c.g *= 0.9;
            c.b *= 0.9;
            c
        }
        button::Status::Pressed => {
            let mut c = design_tokens::color_danger(theme);
            c.r *= 0.8;
            c.g *= 0.8;
            c.b *= 0.8;
            c
        }
        button::Status::Disabled => design_tokens::text_muted(theme),
        _ => design_tokens::color_danger(theme),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            radius: design_tokens::RADIUS_MD.into(),
            ..Default::default()
        },
        shadow: match status {
            button::Status::Hovered => design_tokens::shadow_card(theme),
            _ => iced::Shadow::default(),
        },
        ..Default::default()
    }
}

/// A filled destructive button (danger background, white text).
pub fn destructive_button<'a>(
    label: &'a str,
    on_press: Option<AppMessage>,
    disabled: bool,
) -> Element<'a, AppMessage> {
    let btn = button(
        text(label)
            .font(TypeRole::ButtonLabel.font())
            .size(TypeRole::ButtonLabel.size_px()),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_16])
    .style(button_destructive_style);

    if disabled {
        btn.into()
    } else if let Some(msg) = on_press {
        btn.on_press(msg).into()
    } else {
        btn.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: String) -> AppMessage {
        AppMessage::Noop
    }

    #[test]
    fn form_label_builds() {
        let el: Element<'static, AppMessage> = form_label("Room name");
        let _ = el;
    }

    #[test]
    fn helper_and_error_text_build() {
        let el: Element<'static, AppMessage> = helper_text("This helps");
        let _ = el;
        let el: Element<'static, AppMessage> = error_text("Required");
        let _ = el;
    }

    #[test]
    fn form_section_builds_with_and_without_helper() {
        let el: Element<'static, AppMessage> =
            FormSection::new("Details").build();
        let _ = el;
        let el: Element<'static, AppMessage> = FormSection::new("Details")
            .helper("Fill these in")
            .push(helper_text("child"))
            .build();
        let _ = el;
    }

    #[test]
    fn text_input_builds_all_states() {
        let el: Element<'static, AppMessage> =
            TextInput::new("Name", "…", "", noop).build();
        let _ = el;
        let el: Element<'static, AppMessage> = TextInput::new("Name", "…", "x", noop)
            .helper("Alphanumeric")
            .error("Too short")
            .build();
        let _ = el;
    }

    #[test]
    fn text_area_builds_with_content() {
        let content = text_editor::Content::with_text("hello");
        let el: Element<'_, AppMessage> = TextArea::new(
            "Description",
            &content,
            |_action| AppMessage::Noop,
        )
        .placeholder("Optional…")
        .build();
        let _ = el;
    }

    #[test]
    fn select_builds() {
        let el: Element<'static, AppMessage> = Select::new(
            "Expires after",
            vec!["1h".to_string(), "8h".to_string()],
            Some("1h".to_string()),
            |_| AppMessage::Noop,
        )
        .build();
        let _ = el;
    }

    #[test]
    fn checkbox_and_toggle_build() {
        let el: Element<'static, AppMessage> =
            checkbox_field("Enable DHT", true, |_| AppMessage::Noop, None);
        let _ = el;
        let el: Element<'static, AppMessage> =
            toggle_field("Advertise", false, |_| AppMessage::Noop, Some("helper"));
        let _ = el;
    }

    #[test]
    fn selectable_peer_row_builds() {
        let theme = Theme::Light;
        let el: Element<'static, AppMessage> = SelectablePeerRow::new("Alice")
            .secondary("abc123")
            .selected(true)
            .on_toggle(AppMessage::Noop)
            .build(&theme);
        let _ = el;
        let el: Element<'static, AppMessage> =
            SelectablePeerRow::new("Bob").build(&theme);
        let _ = el;
    }

    #[test]
    fn peer_list_builds_with_rows_and_empty() {
        let theme = Theme::Light;
        let row: Element<'static, AppMessage> =
            SelectablePeerRow::new("Alice").build(&theme);
        let el: Element<'static, AppMessage> =
            peer_list(vec![row], 200.0, Some("No peers available".to_string()));
        let _ = el;
        let el: Element<'static, AppMessage> =
            peer_list(vec![], 200.0, Some("No peers available".to_string()));
        let _ = el;
    }

    #[test]
    fn remove_chip_builds_with_and_without_remove() {
        let el: Element<'static, AppMessage> = remove_chip("Alice", None);
        let _ = el;
        let el: Element<'static, AppMessage> =
            remove_chip("Alice", Some(AppMessage::Noop));
        let _ = el;
    }

    #[test]
    fn selection_summary_singular_and_plural() {
        let el: Element<'static, AppMessage> = selection_summary(1, "participant");
        let _ = el;
        let el: Element<'static, AppMessage> = selection_summary(3, "participant");
        let _ = el;
    }

    #[test]
    fn selectable_peer_list_builds_with_and_without_extras() {
        let theme = Theme::Light;
        let row: Element<'static, AppMessage> =
            SelectablePeerRow::new("Alice").build(&theme);
        let el: Element<'static, AppMessage> =
            SelectablePeerList::new(vec![row], 200.0, Some("No peers available".to_string()))
                .build();
        let _ = el;
        let el: Element<'static, AppMessage> = SelectablePeerList::new(
            vec![SelectablePeerRow::new("Alice").build(&theme)],
            200.0,
            Some("No peers available".to_string()),
        )
        .search("Search…", "", |_| AppMessage::Noop)
        .chips(vec![remove_chip("Alice", Some(AppMessage::Noop))])
        .summary(1, "participant")
        .build();
        let _ = el;
    }

    #[test]
    fn dialog_footer_builds_all_combinations() {
        let el: Element<'static, AppMessage> = DialogFooter::new()
            .cancel("Cancel", AppMessage::Noop)
            .confirm("Create", AppMessage::Noop)
            .build();
        let _ = el;
        let el: Element<'static, AppMessage> = DialogFooter::new()
            .confirm("Create", AppMessage::Noop)
            .confirm_disabled(true)
            .build();
        let _ = el;
        let el: Element<'static, AppMessage> =
            DialogFooter::new().build();
        let _ = el;
    }

    #[test]
    fn destructive_button_builds() {
        let el: Element<'static, AppMessage> =
            destructive_button("Remove", Some(AppMessage::Noop), false);
        let _ = el;
        let el: Element<'static, AppMessage> =
            destructive_button("Remove", None, true);
        let _ = el;
    }
}
