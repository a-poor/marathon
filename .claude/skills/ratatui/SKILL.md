---
name: ratatui
description: >-
  Build terminal user interfaces (TUIs) in Rust with Ratatui 0.30 — the
  immediate-mode render loop, layout/constraints, text & styling, every built-in
  widget, crossterm event handling, text editing (single-line, a from-scratch
  multi-line editor, and LLM-agent chat inputs), and async/tokio architecture.
  Every code block here is verified to compile against ratatui 0.30.1. Use this
  skill whenever the user is writing, reading, debugging, or reviewing Rust code
  involving ratatui or crossterm, a terminal UI / TUI / full-screen terminal
  dashboard, `DefaultTerminal`, `Frame`, the `Widget`/`StatefulWidget` traits,
  `Layout`/`Constraint`/`Rect`, terminal text inputs or key/mouse handling, or
  porting TUI code from ratatui 0.29 to 0.30 — even when the user doesn't say
  "ratatui" by name (e.g. "build me a terminal dashboard in Rust", "add a text
  input box to my full-screen CLI", "why won't my crossterm event loop quit on
  Ctrl-C"). This is the authoritative reference; prefer it over training-data
  memory, which is mostly pre-0.30 and will not compile. Do NOT use for non-Rust
  TUIs (Python Textual, Go Bubble Tea, blessed/ink) or for GPUI / desktop-GUI work.
---

# Ratatui (0.30) — TUI development in Rust

Ratatui is an **immediate-mode** terminal UI library: every frame you rebuild the
whole UI as cheap, short-lived widgets rendered into an in-memory `Buffer`;
ratatui diffs against the previous frame and writes only the changes. There is no
retained tree, no callbacks, no built-in event system — **you** own the loop, the
state, and the input handling.

```text
loop {
    terminal.draw(|frame| render_ui(&app_state, frame))?;   // state -> cells
    handle_events(&mut app_state)?;                          // input -> state
}
```

## Why this skill exists — read before writing code

**0.30 was a major release.** Most ratatui example code on the web and in model
training data targets 0.29 or earlier and **will not compile** against 0.30. When
in doubt, trust the references here over memory, and check the migration list
below. The most common stale-knowledge mistakes:

- `block::Title` is gone → use `Block::title(impl Into<Line>)`, with alignment on
  the `Line` (`Line::from("hi").centered()`), plus `title_top`/`title_bottom`.
- Don't implement `WidgetRef`. Implement `Widget for &MyWidget` (the 0.30 idiom);
  the blanket impl gives you the rest.
- `Style` no longer implements `Styled`; the `Stylize` shortcuts (`.red()`,
  `.bold()`) now exist **directly on `Style`** — no `use Stylize` needed for that.
- `layout::Alignment` → `HorizontalAlignment` (alias kept).
- New conveniences worth adopting: `ratatui::run(..)`, `Rect::layout::<N>(&Layout)`,
  `Rect::centered(h, v)`, `event.is_key_press()` / `event.as_key_press_event()`.

Full migration detail is in `references/01-core-concepts.md` and the per-file
callouts.

## Setup that compiles

The 90% case — depend only on `ratatui` (it re-exports its own crossterm):

```toml
[dependencies]
ratatui = "0.30.1"           # MSRV: Rust 1.88
crossterm = "0.29"           # optional; must match the version ratatui 0.30 uses
```

Verified-compatible ecosystem crates (June 2026): `unicode-width = "0.2"`,
`tui-input = "0.15"` (single-line input), `edtui = "0.11"` (vim-style editor).
**`tui-textarea` is still pinned to ratatui `^0.29`** — using it forces your whole
app onto 0.29; check for a 0.30 release before choosing it.

Smallest possible app (verified):

```rust
fn main() -> std::io::Result<()> {
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|frame| frame.render_widget("Hello! (press any key)", frame.area()))?;
            if crossterm::event::read()?.is_key_press() {
                break Ok(());
            }
        }
    })
}
```

`ratatui::run` sets up the terminal (alt screen + raw mode + panic hook) and
restores it afterward, even on early return.

### Troubleshooting compile errors (learned from compile-testing every snippet)

These three are easy to hit and hard to diagnose from the error message alone:

1. **`E0119` coherence error mentioning `time` and `HourBase`** (e.g. "conflicting
   implementations … `From<…HourBase>` … for `ListItem`/`Cell`"). Plain
   `ratatui = "0.30.1"` compiles fine on its own — this only surfaces when you
   *combine* ratatui's default `widget-calendar` feature (which pulls in `time`)
   with another dependency that compiles `time`'s parsing/formatting modules.
   **`edtui` is the common trigger** (via `syntect → plist → time`); recent `time`
   (≥0.3.47) adds an impl there that conflicts with ratatui-widgets' blanket
   `From<T: Into<Text>>` impls under current rustc. Verified: `ratatui` alone = OK;
   `ratatui` + `edtui` = E0119. Fix: if you don't need the Calendar widget, drop it
   so `time` leaves ratatui-widgets' dependency set —
   ```toml
   ratatui = { version = "0.30.1", default-features = false,
               features = ["crossterm", "layout-cache", "macros", "underline-color"] }
   ```
   and add `tui-input` / `ratatui-widgets` with `default-features = false` too, so
   feature-unification can't switch `widget-calendar` back on.

2. **`row!` macro: "cannot find `ratatui_widgets` in the crate root"**. The
   `ratatui::macros::row!` macro expands to an absolute `::ratatui_widgets::table::Row`
   path (a 0.7.1 macro bug), so it only compiles if `ratatui-widgets` is a **direct**
   dependency. `line!`/`span!`/`text!`/`constraints!` have no such issue. Workaround:
   add `ratatui-widgets` directly, or just build rows with `Row::new([...])`.

3. **`patch_style`/`style`/`centered` "do nothing"**. These are **consuming
   builders** that take `self` by value and *return* the modified value — they are
   not in-place mutations. Use `text = text.patch_style(...)`, not a bare
   `text.patch_style(...);`. (`patch_style` is `#[must_use]`, so the compiler warns.)

## Where to look — routing table

The references are the full, compile-verified documentation. Read the file that
matches the task; don't guess APIs from memory.

| If the task involves… | Read |
|---|---|
| App boot/teardown, the draw loop, event-loop skeletons, viewports (fullscreen/inline/fixed), panic handling, the 0.29→0.30 migration list | `references/01-core-concepts.md` |
| Splitting the screen: `Rect`, `Layout`, `Constraint`, `Flex`, spacing, nesting, grids, centering popups, the layout macros | `references/02-layout.md` |
| `Span`/`Line`/`Text`, `Style`, `Stylize`, `Color`, `Modifier`, masked/password text, the `ratatui-macros` shorthands, unicode width caveats | `references/03-text-and-styling.md` |
| Any built-in widget (Block, Paragraph, List, Table, Tabs, Gauge, Sparkline, Scrollbar, BarChart, Chart, Canvas), popups, or writing a custom `Widget`/`StatefulWidget` | `references/04-widgets.md` |
| Keyboard/mouse/paste/resize/focus events, mode-based dispatch, kitty keyboard protocol, bracketed paste, event→action translation | `references/05-events-and-input.md` |
| Text input of any kind: single-line, a from-scratch multi-line editor, a markdown editor, or an **LLM-agent chat input** (auto-grow, Enter-to-send, Alt+Enter newline, streaming) | `references/06-text-editing.md` |
| Tokio/async integration, background threads + channels, streaming data into the UI, app architecture (Elm/Component), code organization, testing with `TestBackend`/insta | `references/07-async-and-architecture.md` |

## Working habits that keep TUI code correct

- **Filter `KeyEventKind::Press`** (use `event.as_key_press_event()` /
  `is_key_press()`). Windows also delivers `Repeat`/`Release` events; unfiltered
  code acts twice per keystroke. This is the #1 cross-platform TUI bug.
- **In raw mode you own Ctrl-C** — it arrives as a key event, not a signal. Handle
  `(KeyCode::Char('c'), KeyModifiers::CONTROL)` yourself if you want it to quit.
- **The cursor is yours to draw.** For text inputs call
  `frame.set_cursor_position(...)` *every frame* while focused (it's hidden by
  default); don't fake it with a styled block.
- **Cursor math is three coordinate systems**: byte index (for `String` mutation),
  char index (store the cursor here), and display column (for terminal placement —
  CJK/emoji are width 2). Never use `String::len()` for cursor position. Details in
  `references/06-text-editing.md`.
- **Never `println!`/`eprintln!` while the TUI is live** — it corrupts the display.
  Log to a file (tracing) or use an inline viewport. See `references/07`.
- **Render everything every frame**; the diff makes it cheap. Don't try to do
  partial/incremental updates yourself.

When you write non-trivial ratatui code, prefer to actually compile it (a throwaway
`cargo check` against `ratatui = "0.30.1"`) — the gotchas above show the error
messages are often misleading.
