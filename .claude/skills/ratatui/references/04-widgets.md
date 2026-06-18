# Widgets

Built-in widgets with practical snippets, stateful rendering, popups, and custom widgets. Targets ratatui 0.30.

In every snippet `frame: &mut Frame` and `area: Rect`. Widgets are consumed by `render_widget` (they're cheap builders you reconstruct each frame); `&W` also implements `Widget`, so you can render references to widgets you keep in state.

## Block — borders, titles, padding

The universal container; most widgets accept `.block(...)`.

```rust
use ratatui::widgets::{Block, BorderType, Borders, Padding, TitlePosition};
use ratatui::text::Line;
use ratatui::style::{Style, Stylize};

let block = Block::bordered()                          // == Block::new().borders(Borders::ALL)
    .title("Left title")                               // impl Into<Line>  (0.30: no more block::Title!)
    .title(Line::from("Centered").centered())          // alignment lives on the Line
    .title_bottom(Line::from("bottom-right").right_aligned())
    .title_style(Style::new().bold())
    .border_type(BorderType::Rounded)                  // Plain | Rounded | Double | Thick | QuadrantInside | QuadrantOutside
    .border_style(Style::new().cyan())
    .padding(Padding::uniform(1))                      // also ::horizontal, ::vertical, ::new(l,r,t,b), ::proportional
    .style(Style::new().on_black());                   // base style for everything inside

// Render the block, then render content into its inner area:
let inner = block.inner(area);                         // area minus borders/padding/titles
frame.render_widget(block, area);
frame.render_widget(content_widget, inner);
```

Partial borders: `Block::new().borders(Borders::TOP | Borders::BOTTOM)`. Custom border glyphs: `.border_set(ratatui::symbols::border::DOUBLE)` etc. New in 0.30: `.merge_borders(MergeStrategy)` for collapsing adjacent blocks (see 02-layout.md) and `.shadow(Shadow::dark_shade())` (drop shadows for popups).

## Paragraph — text display

```rust
use ratatui::widgets::{Paragraph, Wrap};

let p = Paragraph::new(text)            // impl Into<Text> — String, &str, Line, Vec<Line>, Text…
    .block(Block::bordered().title("Logs"))
    .style(Style::new().white())
    .wrap(Wrap { trim: true })          // word-wrap; trim = strip leading whitespace on wraps
    .scroll((y_offset, x_offset))       // NOTE: (vertical, horizontal) — y first!
    .centered();                        // or .alignment(..) / .left_aligned() / .right_aligned()

frame.render_widget(p, area);
```

- Without `.wrap`, long lines are truncated (and horizontal `.scroll` x applies).
- `.scroll` offsets are in *display* rows (post-wrap) / columns.
- `line_count(width)` / `line_width()` exist but are **unstable** (`unstable-rendered-line-info` feature) — for stable wrapped-height math, do your own wrapping (see 06-text-editing.md) or use the `textwrap` crate.
- A `Paragraph` has no built-in scrollbar — pair with `Scrollbar` (below), tracking scroll state yourself.

## List — selectable rows (stateful)

```rust
use ratatui::widgets::{Block, HighlightSpacing, List, ListItem, ListState};

// In your app state (NOT rebuilt per frame):
//   list_state: ListState   (Default::default())

let items: Vec<ListItem> = app.tasks.iter()
    .map(|t| ListItem::new(Line::from(t.title.as_str())))
    .collect();
let list = List::new(items)                     // also: List::new(["a", "b"]) — Into<ListItem>
    .block(Block::bordered().title("Tasks"))
    .highlight_style(Style::new().reversed())   // style of the selected row
    .highlight_symbol("▶ ")                     // Into<Line> in 0.30
    .highlight_spacing(HighlightSpacing::Always) // reserve symbol column even when nothing selected
    .scroll_padding(2);                         // keep 2 rows visible around the selection

frame.render_stateful_widget(list, area, &mut app.list_state);
```

Selection state API (`ListState`): `select(Some(i))`, `select_next()`, `select_previous()`, `select_first()`, `select_last()`, `selected() -> Option<usize>`, `scroll_down_by(n)`, `scroll_up_by(n)`, `offset()`. The widget auto-scrolls to keep the selection visible. Wiring keys:

```rust
match key.code {
    KeyCode::Down | KeyCode::Char('j') => app.list_state.select_next(),
    KeyCode::Up   | KeyCode::Char('k') => app.list_state.select_previous(),
    KeyCode::Home | KeyCode::Char('g') => app.list_state.select_first(),
    KeyCode::End  | KeyCode::Char('G') => app.list_state.select_last(),
    _ => {}
}
```

Multi-line items work (`ListItem::new(Text::from(vec![line1, line2]))`). `ListDirection::BottomToTop` renders newest-at-bottom feeds. For *widget-valued* items or huge virtual lists, see `tui-widget-list` / build a custom `StatefulWidget`.

## Table — columns + selection (stateful)

```rust
use ratatui::layout::Constraint;
use ratatui::widgets::{Row, Table, TableState};

let header = Row::new(["Name", "Status", "Age"]).style(Style::new().bold()).bottom_margin(1);
let rows = app.servers.iter().map(|s| {
    Row::new([s.name.clone(), s.status.clone(), s.age.to_string()])
});
let table = Table::new(rows, [Constraint::Fill(1), Constraint::Length(10), Constraint::Length(6)])
    .header(header)
    .block(Block::bordered())
    .column_spacing(1)
    .row_highlight_style(Style::new().reversed())
    .highlight_symbol("» ");

frame.render_stateful_widget(table, area, &mut app.table_state);
```

`TableState` mirrors `ListState` and adds column/cell selection: `select_column`, `select_cell`, `selected_cell() -> Option<(row, col)>`, `select_next_column()`, plus `column_highlight_style` / `cell_highlight_style` on the widget. Rows can have `height(n)` for multi-line cells; `Cell::new(...)` accepts styled `Text`.

## Tabs

```rust
use ratatui::widgets::Tabs;

let tabs = Tabs::new(["Overview", "Logs", "Settings"])
    .select(app.active_tab)              // Into<Option<usize>>
    .highlight_style(Style::new().bold().underlined())
    .divider("|");
frame.render_widget(tabs, area);
```

Cycle with `app.active_tab = (app.active_tab + 1) % TAB_COUNT;` then `match` on it to render the body.

## Gauge / LineGauge / Sparkline

```rust
use ratatui::widgets::{Gauge, LineGauge, Sparkline};

frame.render_widget(
    Gauge::default()
        .block(Block::bordered().title("Progress"))
        .gauge_style(Style::new().green().on_black())
        .ratio(0.42)                       // or .percent(42)
        .label("42% (eta 0:31)")
        .use_unicode(true),                // sub-cell resolution
    area);

frame.render_widget(
    LineGauge::default().ratio(0.42).filled_style(Style::new().blue()),
    one_row_area);

frame.render_widget(
    Sparkline::default().data(&app.samples).max(100).style(Style::new().cyan()),
    area);
```

## Scrollbar (stateful, decoration-only)

`Scrollbar` only *draws*; it doesn't scroll anything. You keep the offset, you draw content with that offset, and you mirror it into a `ScrollbarState`:

```rust
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

// app: scroll: usize, and content_len computed from your data
let mut sb_state = ScrollbarState::new(app.content_len.saturating_sub(viewport_height))
    .position(app.scroll);

frame.render_widget(
    Paragraph::new(app.long_text.clone()).scroll((app.scroll as u16, 0)),
    area);
frame.render_stateful_widget(
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑")).end_symbol(Some("↓")),
    area,                                   // often area.inner(Margin::new(0,1)) to avoid block corners
    &mut sb_state);
```

Key/mouse handlers mutate `app.scroll` (clamp to `0..=max`); orientations: `VerticalRight`, `VerticalLeft`, `HorizontalBottom`, `HorizontalTop`.

## Clear + popups

`Clear` resets cells to default before you draw on top — without it, the popup "see-throughs" whatever was beneath:

```rust
use ratatui::widgets::Clear;
use ratatui::layout::Constraint;

if app.show_confirm {
    let popup_area = frame.area().centered(Constraint::Percentage(60), Constraint::Length(7));
    frame.render_widget(Clear, popup_area);
    let block = Block::bordered().title("Confirm").border_type(BorderType::Double);
    let body = Paragraph::new("Delete 3 files? (y/n)").centered().block(block);
    frame.render_widget(body, popup_area);
}
```

Render popups *last* in your draw function (painter's algorithm). While a popup is open, route key events to it first (a simple `enum Focus`/mode field — see the input examples in 06).

## BarChart, Chart, Canvas, Calendar (quick reference)

```rust
// BarChart
use ratatui::widgets::{Bar, BarChart, BarGroup};
let chart = BarChart::default()
    .bar_width(5).bar_gap(1)
    .data(BarGroup::default().bars(&[
        Bar::default().value(42).label("Q1"),     // label: Into<Line> (0.30)
        Bar::default().value(67).label("Q2"),
    ]));

// Chart (line/scatter plots with axes)
use ratatui::widgets::{Axis, Chart, Dataset, GraphType};
use ratatui::symbols::Marker;
let data: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, (i as f64 / 10.0).sin())).collect();
let chart = Chart::new(vec![
        Dataset::default().name("sin").marker(Marker::Braille)
            .graph_type(GraphType::Line).cyan().data(&data),
    ])
    .x_axis(Axis::default().title("t").bounds([0.0, 100.0]).labels(["0", "50", "100"]))
    .y_axis(Axis::default().title("y").bounds([-1.0, 1.0]).labels(["-1", "0", "1"]));

// Canvas (arbitrary shapes in a braille/half-block grid)
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Map, MapResolution, Rectangle};
let canvas = Canvas::default()
    .x_bounds([-180.0, 180.0]).y_bounds([-90.0, 90.0])
    .paint(|ctx| {
        ctx.draw(&Map { resolution: MapResolution::High, color: Color::Green });
        ctx.draw(&CanvasLine { x1: 0.0, y1: 0.0, x2: 40.0, y2: 20.0, color: Color::Red });
    });

// Calendar — enabled by default (via `all-widgets`); needs the `widget-calendar`
// feature only if you build with default-features = false
```

## Custom widgets

### Stateless: implement `Widget` for a reference

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Widget;

struct StatusBar { connected: bool, user: String }

impl Widget for &StatusBar {          // &T so rendering doesn't consume your state (0.30 idiom)
    fn render(self, area: Rect, buf: &mut Buffer) {
        let status = if self.connected { "● online".green() } else { "○ offline".red() };
        Line::from(vec![status, format!(" {}", self.user).into()]).render(area, buf);
    }
}

// usage: frame.render_widget(&app.status_bar, status_area);
```

Don't implement `WidgetRef` (it's unstable & reversed in 0.30); `impl Widget for &T` gives you everything, including rendering from `&self` in composed parents.

### Stateful

```rust
use ratatui::widgets::StatefulWidget;

struct Spinner;                       // widget = config
struct SpinnerState { tick: usize }   // state = persisted between frames

impl StatefulWidget for Spinner {
    type State = SpinnerState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SpinnerState) {
        const FRAMES: [&str; 4] = ["⠋", "⠙", "⠸", "⠴"];
        state.tick = state.tick.wrapping_add(1);
        buf.set_string(area.x, area.y, FRAMES[state.tick / 4 % 4], Style::new());
    }
}
```

### Drawing primitives inside `render`

```rust
buf.set_string(x, y, "text", style);
buf.set_line(x, y, &line, max_width);
buf.set_style(area, style);                       // restyle a region (e.g. selection highlight)
buf[(x, y)].set_symbol("█").set_fg(Color::Red);   // single cell (index by tuple/Position)
let inner = area.inner(Margin::new(1, 1));        // respect your own padding
```

Compose with built-ins: a custom widget's `render` can construct and render `Block`/`Paragraph`/`List` internally (`Widget::render(block, area, buf)`).

### Supporting an optional `.block(...)` like built-ins

```rust
use ratatui::widgets::BlockExt;     // provides Option<Block>::inner_if_some
struct MyWidget<'a> { block: Option<Block<'a>>, /* ... */ }
impl Widget for &MyWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.block.as_ref().render(area, buf);
        let inner = self.block.inner_if_some(area);
        // render content into `inner`
    }
}
```

## Third-party widgets worth knowing

`tui-input` (single-line input), `edtui` (vim-like editor), `tui-textarea` (textarea; 0.29-pinned as of June 2026), `tui-widget-list` (arbitrary widgets as list items), `tui-scrollview` (scrollable container), `tui-popup`, `throbber-widgets-tui` (spinners), `ratatui-image` (images via sixel/kitty/iterm2), `tui-markdown` (markdown → Text), `tachyonfx` (shader-like effects), `tui-logger`, `tui-tree-widget`. Check each against your ratatui version. Full list: https://ratatui.rs/showcase/third-party-widgets/
