# Visual designer scope

This document records the schema-driven designer coverage after the Home-first integration pass. Designer controls remain behind the `dev-ui` feature; normal builds do not create or apply designer state.

## Home: supported

The Home screen consumes the live `LayoutConfig.home` model and rebuilds when its revision changes. The following edits are semantic and survive TOML save/reload:

- top-level section order (`section_order`), including drag-handle drops and tree up/down controls;
- section visibility (`hidden_sections`), with recovery controls in the layout inspector;
- Quick Actions responsive column counts and breakpoints, card padding, and grid gap;
- the Home grid/list/row mode, main/rail proportions, column gap, and stacking breakpoint;
- the Home canvas max width and responsive horizontal padding;
- page, section, card, and footer spacing (`home.gaps` and `home.padding`);
- Home card sizing constraints, including peer/activity sizing, status-card constraints, and Quick Actions icon size;
- responsive preview widths and per-tier Home columns/padding.

The existing production widgets are wrapped by developer-only overlays. Dragging uses explicit handles, and pointer coordinates are used only during the gesture; only section indices or typed layout values are persisted. Rejected drops and out-of-range resizes leave the last valid layout active and report the reason in the designer error banner.

The Home section IDs retain the production card identity: `MeshHealth` is the current Public Rooms/mesh status section and `PeopleActivity` is the current Friends/people section. They are not duplicate widgets or a second persistence model.

## Screen extension status

| Screen | Status | Schema-driven coverage |
|---|---|---|
| Sidebar | Modelled, not yet rendered from the model | `LayoutConfig.sidebar` contains width constraints, section order/visibility, padding, and row heights. The current sidebar view is intentionally not exposed as editable until it consumes these fields. |
| Chat | Incremental designer support | `LayoutConfig.chat` is used for the supported message-list and composer resize overlays. Bubble, picker, details, member-list, and screen-share fields remain modelled but are not advertised as editable until their production renderers consume them. |
| Attachments / file cards | Reused component architecture | `LayoutConfig.component.video` and `component.shared_by_me` are consumed by the production video/file card and sharing-table renderers. No raw coordinates are stored. |
| Rooms | Not modelled as a screen layout | Room content uses the existing Home/sidebar/chat surfaces; no room-specific designer controls are added. |
| Tunnels | Home card only | Tunnel card visibility/order follows Home section semantics. There is no separate tunnel-screen layout in `LayoutConfig`, so tunnel-dialog geometry is not exposed. |
| Dialogs | Not modelled as a screen layout | Dialogs continue to use their production layout and theme tokens. No pixel-position persistence or duplicate editable dialogs were introduced. |

Future screen work should first add a typed schema and wire the production renderer to it, then expose the field through the existing inspector/merge/watcher path. Do not add designer controls for an unconsumed field and do not persist desktop coordinates.

## Verification notes

Mechanical verification for this pass is performed with the repository's `rb` wrapper on DEBSRV. Manual visual acceptance (reorder, visibility recovery, Quick Actions columns/gap, save/reload, narrow/maximized windows, and Designer Mode disablement) remains a human-batched step.
