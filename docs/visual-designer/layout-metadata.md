# Visual designer layout metadata

The developer-only `layout_metadata` module is a runtime projection of the
typed `LayoutConfig`. It gives the designer a stable registry without making
the Iced view tree, widget state, or raw desktop coordinates a persistence
format.

## API

`LayoutMetadata::from_layout(&active_layout)` returns metadata for every stable
`designer::ComponentId`. `LayoutMetadata::with_bounds` accepts a callback for
transient bounds collected by the view layer, and `metadata_for` is the
single-component accessor. Bounds are optional and are never serialized.

Each `ComponentMeta` exposes:

- `component_id` and `parent_layout_id` (`home`, `sidebar`, or `chat`)
- optional current `Bounds`
- applicable `LayoutOperation` values
- display properties keyed by the authoritative `LayoutConfig` path
- numeric min/max `Constraint` values for resize affordances

Property strings are read-only display metadata. An editor operation must update
the existing typed layout override (`LayoutOverrides`) and then use the normal
merge, validation, watcher, and inspector paths. This prevents metadata from
becoming a second persistence system.

## Component mapping

| Component ID | Parent | Authoritative layout fields | Operations |
|---|---|---|---|
| `home.welcome` | `home` | `home.section_order`, `home.hidden_sections` | reorder, visibility |
| `home.quick_actions` | `home` | `home.quick_actions.columns_*`, `home.gaps` | reorder, columns, visibility, spacing |
| `home.public_rooms` | `home` | `home.section_order`, `home.hidden_sections` | reorder, visibility |
| `home.friends` | `home` | `home.section_order`, `home.hidden_sections` | reorder, visibility |
| `home.recent_activity` | `home` | `home.section_order`, `home.hidden_sections` | reorder, visibility |
| `sidebar` | `sidebar` | `sidebar.width`, `sidebar.width_min`, `sidebar.width_max`, `sidebar.section_order`, `sidebar.padding` | resize width, reorder, visibility, spacing |
| `chat.message_list` | `chat` | `chat.bubble_max_width`, `chat.bubble_width_ratio`, `chat.message_max_width` | resize width/height, alignment, spacing |
| `chat.composer` | `chat` | `chat.composer.button_order`, `chat.composer.spacing`, `chat.composer.padding` | reorder, resize width, orientation, alignment, spacing |

The operations list is deliberately limited to capabilities represented by
the existing typed model. No freeform x/y movement is advertised for
responsive content.

The module is compiled only with `dev-ui`; normal Boru builds and runtime
behaviour are unchanged.