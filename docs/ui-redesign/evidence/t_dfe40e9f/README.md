# UI-HOME-15 responsive evidence

Four supported window widths captured from the running Boru GUI under Xvfb
(fresh data dir, --no-dht --no-relay, MCP-driven home navigation). The
dashboard breakpoints are content-width based: window minus sidebar (288-320
px), divider and page padding.

| Width | Content width (approx) | Intentional layout |
|---|---|---|
| 1600x900 | ~1231 px | Two dashboard columns, four quick-action columns, full hero illustration |
| 1280x800 | ~919 px | Two dashboard columns, 2x2 quick actions |
| 1024x720 | ~679 px | One dashboard column (right rail below), 2x2 quick actions, scaled illustration |
| 800x600 | ~455 px | One dashboard column, one quick action per row, compact headers, pill under greeting, no illustration |

`*_scrolled.png` captures are taken after mouse-wheel scrolling down over the
main panel, so the second quick-action row (1280/1024) and the full stacked
layout (800) are visible.

See docs/ui-redesign/UI-HOME-15-report.md for the full report and
geometry.txt for OCR word-box overflow checks.
