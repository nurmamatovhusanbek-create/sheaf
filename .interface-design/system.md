# Design System: Sheaf

## Direction

**Who:** someone at a desk with a stack of paper and a scanner (an office, a home, a school), tidying documents: merge, reorder, compress, recognize, send. They are not technical; they want to see the page and know what will happen.
**Feel:** calm and tactile, like a clean desk. Warm, quiet, one clear action.
**Personality:** Utility and warmth. Dense enough to work in, never cold.
**Foundation:** warm neutrals (one hue family, only lightness changes).
**Depth:** tone plus hairlines. The *page* is the only thing that casts a shadow.

**Domain:** sheaf, ream, tab divider, stapler, stamp, carbon copy, filing, guillotine, ink.
**Color world:** paper white, manila, desk blotter, fountain-pen ink blue, stamp pad, graphite.
**Signature:** the page lies on a darker desk with a real shadow, and the empty state is a stack of paper. Nothing else in the window casts a shadow.
**Rejected defaults:** dark slate + bright blue dashboard → warm paper neutrals + ink blue; a grid of big tool cards → a short grouped list; two rows of controls above the preview → one slim bar and a palette floating at the foot of the desk.

## Tokens (crates/sheaf-ui/src/theme.rs, `Palette`)

| token | light | dark | use |
|---|---|---|---|
| desk | #e3dfd4 | #141311 | preview background only |
| panel | #f5f2eb | #1d1c19 | toolbar, tray, side panel (one tone) |
| surface | #fcfaf6 | #262521 | dialogs, menus, the floating palette |
| inset | #ebe7dc | #161513 | fields (a step *below* their surroundings) |
| hover / pressed | #e9e5da / #ded9cb | #2d2b27 / #373530 | states |
| line / line_strong | rgba(60,45,10,.12 / .24) | rgba(255,244,222,.09 / .18) | hairlines |
| text / weak / faint | #25221b / #625c4f / #a9a191 | #ece8de / #a69f90 / #635e54 | ramp: primary, supporting, disabled |
| accent | #2a499c | #6f8be8 | primary action, selection, active tab/chip |
| ok / warn / bad | #2f7d52 / #a86a10 / #b83a28 | #5bb381 / #d9a441 / #e5766a | status (always with words) |

Spacing: 4-point grid (4 related, 8 groups, 12/16 blocks). Radius: 5 controls, 8 rows/menus, 12 dialogs.
Type: Noto Sans 11.5 / 13 / 18 (one weight bundled: hierarchy = size + colour ramp).
Control height 26 (toolbar buttons 28, primary 28 to 30).

## Patterns

- **Primary button:** accent fill, on_accent text, 28 high (dialog footer, right-aligned) or full width in a side form (30). One per view.
- **Chip / segmented:** 24 high, 5 radius; on = accent wash (alpha 34 light, 56 dark) with accent text; segmented groups sit in an inset frame.
- **Tool row:** 26 high, icon in `weak`, accent only on hover; dimmed with a reason when a document is needed.
- **Job row:** 124 label, 150×6 track, 38 mono percentage, summary; moving segment when the length is unknown.
- **Dialog:** field labels small and `weak` *above* their controls; hairline then footer with secondary actions and the one primary on the right.
- **Page on the desk:** `theme::paper_shadow` (a soft depth plus a contact shadow).

## Decisions

| Decision | Why |
|---|---|
| The page is the focal point | It is the product; light theme had a white page on a near-white ground |
| One accent, scarce | Sixteen blue icons and a blue tab pill made the accent meaningless |
| Badge the exception | A text layer is normal; a page *without* one is what OCR fixes |
| Help, updates, About, report behind one `?` | Three are used rarely; they do not earn a permanent button each |
| Contrast is tested | The palette is a table in `theme.rs`; `cargo test` checks it against every surface |

## Known limits

- One font weight is bundled, so emphasis cannot be bold; if a bold Noto is added, use it for dialog titles and the focal numbers.
- egui has no tabular figures; percentages and page counts use the monospace face to stop jitter.
- No motion beyond the moving progress segment (egui animates nothing by default).
