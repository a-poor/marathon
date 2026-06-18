# Layout

Splitting the screen into regions. Targets ratatui 0.30.

## `Rect` — the unit of placement

Everything renders into a `Rect { x, y, width, height }` (all `u16`, in character cells; origin top-left). Useful methods:

```rust
use ratatui::layout::{Rect, Margin, Offset, Position, Constraint};

let area = frame.area();                       // whole screen
area.inner(Margin::new(2, 1));                 // shrink by horizontal=2, vertical=1
area.offset(Offset::new(3, -1));               // shift (also: rect + Offset)
area.contains(Position::new(x, y));            // hit-testing (mouse!)
area.intersection(other); area.union(other);
area.is_empty(); area.left(); area.right(); area.top(); area.bottom();
area.centered(Constraint::Percentage(60), Constraint::Length(9));   // centered sub-rect (popups)
area.centered_horizontally(Constraint::Length(30));
area.centered_vertically(Constraint::Percentage(50));
```

## `Layout` — constraint-based splitting

```rust
use ratatui::layout::{Constraint, Layout};

// Build once, apply to an area. Two equivalent application styles:
let layout = Layout::vertical([
    Constraint::Length(3),   // header: exactly 3 rows
    Constraint::Min(0),      // body: everything left
    Constraint::Length(1),   // status line
]);

// (a) Destructure into a fixed-size array (preferred — compile-time count check):
let [header, body, status] = layout.areas(frame.area());
// (b) 0.30 ergonomic equivalent, callable on the Rect:
let [header, body, status] = frame.area().layout(&layout);
// (c) Dynamic count -> Rects (derefs to &[Rect]):
let rows = layout.split(frame.area());
// (d) Dynamic count, owned Vec: frame.area().layout_vec(&layout)
```

`Layout::horizontal([...])` splits left→right. Builder extras:

```rust
Layout::vertical([Constraint::Length(1); 3])
    .margin(1)                 // outer margin all sides (also .horizontal_margin/.vertical_margin)
    .spacing(1)                // gap between segments; .spacing(-1) overlaps borders by 1 (Spacing::Overlap)
    .flex(ratatui::layout::Flex::Start);
```

`areas::<N>()` panics if `N` doesn't match the constraint count (use `try_areas` for a `Result`). `spacers::<N+1>()`/`split_with_spacers` return the gap rects too (useful for drawing separators in the gaps).

## `Constraint` variants and their meaning

```rust
use ratatui::layout::Constraint::*;
```

| Constraint | Meaning |
|---|---|
| `Length(u16)` | exactly N cells (highest practical priority) |
| `Percentage(u16)` | % of the area |
| `Ratio(u32, u32)` | fraction of the area, e.g. `Ratio(1, 3)` |
| `Min(u16)` | at least N; greedily absorbs extra space (weaker than `Fill`) |
| `Max(u16)` | at most N |
| `Fill(u16)` | take remaining space, weighted: `Fill(2)` gets 2× a `Fill(1)` |

Priority when space is short (highest first): `Min`/`Max` bounds, then `Length`, then `Percentage`/`Ratio`, then `Fill`/`Min`-growth. Rounding can shift a cell between siblings; don't depend on exact pixel math across resizes.

Bulk constructors: `Constraint::from_lengths([1,1,1])`, `from_percentages([50,50])`, `from_ratios(..)`, `from_mins(..)`, `from_maxes(..)`, `from_fills(..)`.

Typical recipes:

```rust
// Sidebar + main:
let [sidebar, main] = Layout::horizontal([Length(24), Fill(1)]).areas(area);

// Header / content / footer:
let [top, mid, bottom] = Layout::vertical([Length(3), Fill(1), Length(1)]).areas(area);

// Two equal columns with a gap:
let [left, right] = Layout::horizontal([Fill(1), Fill(1)]).spacing(2).areas(area);

// Fixed-width centered column:
let column = area.centered_horizontally(Length(80));
```

## `Flex` — distributing leftover space

When constraints don't consume the full area, `Flex` decides where the slack goes (mirrors CSS flexbox):

```rust
use ratatui::layout::Flex;
let layout = Layout::horizontal([Length(10), Length(10)]).flex(Flex::Center);
```

- `Flex::Legacy` — old tui-rs behavior: last element stretches.
- `Flex::Start` (default) — pack at start; slack at the end.
- `Flex::End`, `Flex::Center`
- `Flex::SpaceBetween` — slack between items only.
- `Flex::SpaceAround` — gaps around items; middle gaps are 2× the outer ones *(changed in 0.30 to match flexbox)*.
- `Flex::SpaceEvenly` — all gaps equal *(the pre-0.30 `SpaceAround` behavior)*.

`Flex` + fixed-size constraints replaces most manual centering math:

```rust
// A 30x9 box dead-center (equivalent to area.centered(..)):
let [h] = Layout::horizontal([Length(30)]).flex(Flex::Center).areas(area);
let [popup] = Layout::vertical([Length(9)]).flex(Flex::Center).areas(h);
```

## Nesting layouts

Compose splits hierarchically — split, then split the pieces:

```rust
let [top, bottom] = Layout::vertical([Percentage(50), Percentage(50)]).areas(frame.area());
let [bottom_left, bottom_right] = Layout::horizontal([Percentage(50), Percentage(50)]).areas(bottom);
```

### Grid

```rust
fn grid(area: Rect, rows: u16, cols: u16) -> Vec<Rect> {
    let row_rects = Layout::vertical(vec![Constraint::Ratio(1, rows as u32); rows as usize]).split(area);
    row_rects
        .iter()
        .flat_map(|&row| {
            Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols as usize])
                .split(row)
                .to_vec()
        })
        .collect()
}
```

## Dynamic / data-driven layouts

Constraints are values — compute them at render time:

```rust
// One row per item, then slack:
let mut constraints: Vec<Constraint> = items.iter().map(|_| Constraint::Length(1)).collect();
constraints.push(Constraint::Fill(1));
let areas = Layout::vertical(constraints).split(area);

// Input box that grows with content (see 06-text-editing.md):
let input_height = (line_count as u16).clamp(1, 6) + 2 /* borders */;
let [chat, input] = Layout::vertical([Fill(1), Length(input_height)]).areas(area);
```

## Collapsing borders between adjacent blocks

Adjacent `Block::bordered()` widgets produce doubled borders. Options:

1. `Layout::spacing(-1)` overlaps the rects by one cell (`Spacing::Overlap(1)`), and `Block::merge_borders(MergeStrategy::Exact)` (0.30) merges the overlapping border glyphs cleanly (`Exact` merges only exactly-matching line styles; `Fuzzy` merges approximately across styles; `Replace` just overwrites):

```rust
use ratatui::symbols::merge::MergeStrategy;
use ratatui::widgets::Block;
let [left, right] = Layout::horizontal([Fill(1), Fill(1)]).spacing(-1).areas(area);
frame.render_widget(Block::bordered().merge_borders(MergeStrategy::Exact), left);
frame.render_widget(Block::bordered().merge_borders(MergeStrategy::Exact), right);
```

2. Manual: give one block `Borders::ALL` and the neighbor `Borders::TOP | Borders::RIGHT | Borders::BOTTOM`, picking joint border-set symbols (the classic recipe: https://ratatui.rs/recipes/layout/collapse-borders/).

## The `constraints!` / `vertical!` / `horizontal!` macros

With the `macros` feature (or `ratatui-macros` crate):

```rust
use ratatui::macros::{constraints, horizontal, vertical};

let [a, b, c] = area.layout(&vertical![==3, *=1, ==1]);          // Length(3), Fill(1), Length(1)
let [l, r]    = area.layout(&horizontal![==30%, >=20]);          // Percentage(30), Min(20)
let cs = constraints![==5, ==20%, >=3, <=10, ==1/4, *=1];        // the full syntax menu
```

Syntax: `==N` Length, `==N%` Percentage, `==A/B` Ratio, `>=N` Min, `<=N` Max, `*=N` Fill.

## Layout cache

Solving constraints uses the `kasuari` (cassowary) solver; results are memoized per (Rect, Layout) in a thread-local LRU cache (`layout-cache` default feature, size tunable via `Layout::init_cache`). Practical upshot: reusing the same `Layout` values is effectively free; generating *unique* constraint sets every frame defeats the cache but is still usually fast enough.
