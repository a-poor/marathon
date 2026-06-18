# Text and Styling

`Span` → `Line` → `Text`, plus `Style`, `Color`, `Modifier`. Targets ratatui 0.30.

## The text hierarchy

| Type | Is | Renders as |
|---|---|---|
| `Span<'a>` | one `Cow<str>` + one `Style` (no newlines!) | inline run of styled chars |
| `Line<'a>` | `Vec<Span>` + optional style + alignment | exactly one terminal row |
| `Text<'a>` | `Vec<Line>` + optional style + alignment | multiple rows |

All three implement `Widget` (renderable directly) and convert into each other / from strings:

```rust
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};

// Spans
let s1 = Span::raw("plain");
let s2 = Span::styled("loud", Style::new().fg(Color::Red).add_modifier(Modifier::BOLD));
let s3 = "shortcut".red().bold();          // Stylize on &str -> Span

// Lines: from a str, or by composing spans
let l1 = Line::from("one row");
let l2 = Line::from(vec!["Hello ".into(), "World".bold().yellow()]);
let l3 = Line::from_iter(["a", "b"]);      // also FromIterator<Span>
let l4 = Line::raw("no-style");

// Text: from strings (splits on \n), lines, or iterators
let t1 = Text::raw("two\nrows");
let t2 = Text::from(vec![l1, l2]);
let t3 = Text::from_iter(["row 1", "row 2"]);
```

Lifetimes: `'a` borrows the source string. Use owned `String`s (or `.to_string()`) when the text outlives the data, or build per-frame from `&self` state (the normal immediate-mode pattern — borrowing app state for the duration of `render` is idiomatic and free).

Useful methods:

```rust
line.width();  text.width();  text.height();        // display width in cells (unicode-aware)
line.push_span("more");  text.push_line("more");  text.push_span("tail");
line.centered(); line.left_aligned(); line.right_aligned();   // alignment baked into the value
text.centered();                                              // ditto for all lines
let line = line.style(Style::new().italic());                 // base style for the whole line
let text = text.patch_style(Style::new().dim());              // merge style in (consuming builder: returns the patched value, #[must_use])
let spans: Vec<&Span> = line.iter().collect();                // Line iterates over spans, Text over lines
```

Alignment set on a `Line`/`Text` survives into widgets — e.g. `Block::title(Line::from("hi").centered())` centers the title; `Paragraph::new(text)` respects per-line alignment.

## `Style`

```rust
use ratatui::style::{Color, Modifier, Style};

Style::new()                       // == Style::default()
    .fg(Color::Cyan)
    .bg(Color::Black)
    .underline_color(Color::Red)   // `underline-color` feature (default on); crossterm/termwiz backends only
    .add_modifier(Modifier::BOLD | Modifier::ITALIC)
    .remove_modifier(Modifier::DIM);

// Stylize shortcuts work directly on Style (0.30: Style no longer implements Styled):
Style::new().red().on_black().bold().not_dim();

// Combine: `patch` merges (rhs wins where set); fields are Options under the hood.
let combined = base_style.patch(overlay_style);
Style::reset()                     // explicit "clear everything" style
```

Styling is **layered**: widget style → block style → line style → span style, later/inner layers override earlier/outer ones per-field (fg, bg, each modifier independently). So a `Paragraph::style(Style::new().white())` with one `"err".red()` span gives white text with one red word.

`Modifier` bitflags: `BOLD`, `DIM`, `ITALIC`, `UNDERLINED`, `SLOW_BLINK`, `RAPID_BLINK`, `REVERSED`, `HIDDEN`, `CROSSED_OUT`. Terminal support varies (italic and blink are the usual casualties); test in your target terminals.

## `Stylize` — the fluent shortcuts

`use ratatui::style::Stylize;` adds chainable methods to **strings, spans, lines, text, and widgets** (anything implementing `Styled`):

```rust
"hello".red();  "hello".on_blue();  "hello".bold().italic().underlined();
"hello".gray(); "hello".dark_gray(); "hello".light_blue();  // all 16 ANSI names
"hello".not_bold();                                          // removers
Line::from("x").yellow();  Paragraph::new("x").on_black();   // widgets too
```

Color methods: `black, red, green, yellow, blue, magenta, cyan, gray, dark_gray, light_red, light_green, light_yellow, light_blue, light_magenta, light_cyan, white` + `on_*` variants.

## `Color`

```rust
use ratatui::style::Color;

Color::Red                          // 16 ANSI colors — adapt to the user's palette
Color::Indexed(208)                 // 256-color palette
Color::Rgb(255, 128, 0)             // truecolor
Color::from_u32(0x00FF8000)         // 0x00RRGGBB
"#FF8000".parse::<Color>()?;        // FromStr: hex, names ("red"), indexed ("10")
Color::Reset                        // terminal default fg/bg
```

Prefer ANSI named colors for chrome (respects user themes, works everywhere); use RGB for data viz. Windows legacy console and some terminals lack truecolor; `Color::Rgb` degrades unpredictably there. With the `palette` feature you can convert from `palette` crate types.

## Masked text (passwords)

```rust
use ratatui::text::Masked;
let password = Masked::new("hunter2", '*');     // Into<Text>, renders *******
Paragraph::new(password);
```

## `ratatui-macros` shorthands (`macros` feature)

```rust
use ratatui::macros::{line, span, text, row};

let s = span!("plain");
let s = span!("{value:.2}");                          // format!-style
let s = span!(Style::new().bold(); "styled {}", x);   // style; format
let l = line!["Hello ", span!(Modifier::BOLD; "World")];   // Vec<Span>-like
let l = line!["-"; 3];                                // repeat: "-", "-", "-"
let t = text![line!["a"], line!["b"]];                // Vec<Line>-like
let r = row![cell_a, cell_b];                         // table rows
// plus constraints!/vertical!/horizontal! — see 02-layout.md
```

> ⚠️ `row!` is the one exception: in `ratatui-macros` 0.7.1 it expands to an
> absolute `::ratatui_widgets::table::Row` path, so it only compiles if you add
> `ratatui-widgets` as a **direct** dependency (`line!`/`span!`/`text!` resolve
> through `ratatui-core` and need no extra dep). If you don't want that dep,
> build rows with `Row::new([...])` directly.

## Unicode caveats (important for editors — see 06)

- Cell ≠ char ≠ byte. CJK and emoji are **2 cells wide**; combining marks are 0. `Span::width()`/`Line::width()` measure display cells via `unicode-width`.
- `String::len()` is bytes; never use it for cursor math. Use `chars().count()` for char positions and `unicode_width::UnicodeWidthStr::width(s)` for display columns.
- Grapheme clusters (e.g. 👩‍👩‍👧‍👦, flags) span multiple chars; for cursor-correct editors iterate with `unicode-segmentation`'s `graphemes()`.
- Ratatui handles wide chars in the buffer correctly (the second cell is skipped), but *your* cursor positioning and truncation logic must account for widths.

## Quick recipes

```rust
// Key-hint footer line:
let hints = Line::from(vec![
    " q ".bold().reversed(), " quit  ".into(),
    " ↑/↓ ".bold().reversed(), " select ".into(),
]).centered();

// Log line with level coloring:
fn level_span(level: &str) -> Span<'_> {
    match level {
        "ERROR" => level.red().bold(),
        "WARN"  => level.yellow(),
        "INFO"  => level.green(),
        _        => level.dim(),
    }
}

// Diff-style text:
let diff = Text::from(vec![
    Line::from("+ added line").green(),
    Line::from("- removed line").red(),
]);
```
