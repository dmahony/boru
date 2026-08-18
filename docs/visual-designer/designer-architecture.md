# Visual designer architecture

## Purpose and boundary

The visual designer is a developer-only overlay on the normal Boru Iced application. Its transient state lives in `src/bin/boru/designer.rs` as `DesignerState`; it is stored as a separate field on `IcedChat`, rather than inside chat, room, network, tunnel, media, transfer, or persistence state.

`DesignerState` contains only editor concerns:

- whether designer mode is enabled;
- hovered and selected component names;
- the active pointer-session drag and resize operations;
- the current responsive preview breakpoint;
- a dirty flag; and
- validation error messages.

Drag and resize points describe the current pointer session only. They are not a persistence format and must not become raw desktop coordinates in layout files. Later designer work will translate operations into the existing typed layout/theme models.

## Message flow

Designer actions use the normal Iced message path:

1. A developer-only control or future editable widget emits an `AppMessage::Designer(DesignerMessage)`.
2. `IcedChat::update` receives that message alongside ordinary application messages.
3. The update arm delegates to `DesignerState::update`.
4. The reducer changes only the designer overlay and returns `iced::Task::none()`; it does not restart services or mutate production application state.

The message set already covers entering and exiting mode, hover and selection, starting/updating/cancelling drag and resize operations, breakpoint selection, dirty-state marking, and validation-error management. Rendering, hit-testing, stable component IDs, layout metadata, persistence, and inspector synchronization are intentionally subsequent tasks.

## Developer gate

The module, state field, message variant, and update arm are all guarded by Cargo's existing `dev-ui` feature. A default-features build therefore has no designer state or designer messages and follows the existing application path unchanged. Runtime debug gating remains the responsibility of the existing developer-UI gate; enabling the feature is the explicit build-time opt-in for the developer surface.

The designer reuses the existing Boru theme/layout infrastructure. It does not introduce a second TOML file, persistence store, or application-state owner.
