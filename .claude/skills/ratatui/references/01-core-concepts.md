# Core Concepts

How a ratatui app boots, draws, and shuts down. Targets ratatui 0.30.

## The mental model

- **Immediate mode**: every frame you construct widgets from scratch and render them. Widgets are cheap value types (builders); rendering writes styled characters ("cells") into a `Buffer`.
- **Double buffering**: `Terminal` keeps two buffers; after your draw closure runs, it diffs current vs previous and emits only the changed cells to the terminal, then swaps. You never "clear and redraw" manually — just describe the full UI each frame.
- **You own the loop**: ratatui has no event system. You read events (usually via crossterm), mutate your state, and draw again.
- **State lives in your structs**: widgets don't hold state across frames. The few "stateful" widgets (`List`, `Table`, `Scrollbar`) take a `&mut State` you store yourself.

## Terminal setup and teardown

All of these live at the crate root (`ratatui::*`) and return/operate on `DefaultTerminal = Terminal<CrosstermBackend<Stdout>>`.

| Function | Alt screen | Raw mode | Errors | Use |
|---|---|---|---|---|
| `ratatui::run(f)` | ✓ | ✓ | auto-cleanup | the default choice |
| `ratatui::init()` | ✓ | ✓ | panics | manual control |
| `ratatui::try_init()` | ✓ | ✓ | `Result` | manual + error handling |
| `ratatui::init_with_options(opts)` | ✗ | ✓ | panics | inline/fixed viewports |
| `ratatui::try_init_with_options(opts)` | ✗ | ✓ | `Result` | same, with `Result` |
| `ratatui::restore()` | — | — | prints to stderr | pair with `init` |
| `ratatui::try_restore()` | — | — | `Result` | pair with `try_init` |

**Raw mode** disables line buffering, echo, and Ctrl-C signal handling — you receive every key press as an event (including `Ctrl+C`, which you must handle yourself if you want it to quit). **Alternate screen** is a second terminal buffer; your UI doesn't disturb the user's scrollback and disappears on exit.

All init functions **install a panic hook** that restores the terminal before the panic message prints, so a panicking app doesn't leave the terminal in a broken state. If you install your own hooks (e.g. `color_eyre::install()`), do so *before* calling `init`/`run`.

### `ratatui::run` — the 90% case

```rust
pub fn run<F, R>(f: F) -> R
where F: FnOnce(&mut DefaultTerminal) -> R
```

Note it's generic over the return value — your closure can return `Result<Option<Form>, E>` or anything else:

```rust
fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;                     // BEFORE run(), so the hook chain is right
    ratatui::run(|terminal| App::new().run(terminal))
}
```

### Manual `init`/`restore` — when you need control

Use when the terminal must outlive a single closure (e.g. async main, or you suspend the TUI to spawn `$EDITOR`):

```rust
fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal);
    ratatui::restore();
    result   // restore BEFORE printing errors, or they'll be invisible/garbled
}
```

## Drawing: `Terminal::draw` and `Frame`

```rust
terminal.draw(|frame: &mut Frame| {
    frame.render_widget(some_widget, some_area);
})?;   // -> io::Result<CompletedFrame>
```

One `draw` call = one complete frame. Render *everything* every time; the diff makes it cheap. Key `Frame` methods:

- `frame.area() -> Rect` — full drawable area (use this, not terminal size). *(pre-0.28: `frame.size()`, now deprecated)*
- `frame.render_widget(widget, area)` — render any `Widget`.
- `frame.render_stateful_widget(widget, area, &mut state)` — for `List`/`Table`/`Scrollbar`/custom.
- `frame.set_cursor_position(impl Into<Position>)` — show the hardware cursor at a cell this frame (hidden by default; must be called **every frame** you want it visible). Essential for text inputs — see [06-text-editing.md](06-text-editing.md).
- `frame.buffer_mut() -> &mut Buffer` — direct cell access (custom effects, low-level drawing).
- `frame.count() -> usize` — frame counter (animations).

`Widget` is implemented for many plain types, so these all work directly: `frame.render_widget("hello", area)`, `String`, `Line`, `Text`, `Span` — and for `&W` references of all built-in widgets (plus any custom widget that follows the `impl Widget for &MyWidget` idiom).

`terminal.draw` also handles resizes: it queries the size each frame and auto-resizes buffers. On `Event::Resize` you just need to loop around and draw again.

Use `terminal.try_draw(|f| -> io::Result<()> { ... })` if rendering itself can fail and you want `?` inside the closure.

## Event-loop skeletons

### 1. Draw-on-event (blocking) — simplest, zero idle CPU

Good for forms, pickers, dashboards that change only on input:

```rust
use crossterm::event::{self, Event, KeyCode, KeyEventKind};

fn run(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    loop {
        terminal.draw(render)?;                 // draw, then block on input
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                _ => { /* update state */ }
            },
            Event::Resize(_, _) => {}           // loop redraws at new size
            _ => {}
        }
    }
}
```

Crossterm 0.29 helpers shorten the common filter (`KeyEventKind::Press` filtering matters: Windows also delivers `Repeat`/`Release` events, so unfiltered code double-triggers):

```rust
if let Some(key) = event::read()?.as_key_press_event() {
    match key.code { /* ... */ }
}
// or: if event::read()?.is_key_press() { ... }
```

### 2. Tick-based (poll with timeout) — animations / background updates

```rust
use std::time::{Duration, Instant};
use crossterm::event;

const TICK_RATE: Duration = Duration::from_millis(100);

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    let mut last_tick = Instant::now();
    while !app.should_quit {
        terminal.draw(|frame| app.render(frame))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {                       // wait for input OR timeout
            let event = event::read()?;                  // never blocks after poll()==true
            app.handle_event(&event);
        }
        if last_tick.elapsed() >= TICK_RATE {
            app.on_tick();                               // advance animations, poll jobs
            last_tick = Instant::now();
        }
    }
    Ok(())
}
```

Drain *all* pending events before redrawing if you expect bursts (e.g. mouse drags): `while event::poll(Duration::ZERO)? { app.handle_event(&event::read()?); }`.

### 3. Async (tokio + `EventStream`) — see [07-async-and-architecture.md](07-async-and-architecture.md)

```rust
// crossterm = { version = "0.29", features = ["event-stream"] }
let mut events = crossterm::event::EventStream::new();
let mut frames = tokio::time::interval(Duration::from_secs_f32(1.0 / 60.0));
loop {
    tokio::select! {
        _ = frames.tick() => { terminal.draw(|f| app.render(f))?; }
        Some(Ok(event)) = events.next() => app.handle_event(&event),
    }
}
```

## Structuring an app

The canonical shape — state struct, `run` loop method, separate render/update paths:

```rust
fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    ratatui::run(|terminal| App::default().run(terminal))
}

#[derive(Default)]
struct App {
    should_quit: bool,
    counter: i64,
}

impl App {
    fn run(mut self, terminal: &mut ratatui::DefaultTerminal) -> color_eyre::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        use ratatui::text::Line;
        let text = Line::from(format!("Counter: {} (j/k to change, q to quit)", self.counter));
        frame.render_widget(text.centered(), frame.area());
    }

    fn handle_events(&mut self) -> std::io::Result<()> {
        use crossterm::event::{self, KeyCode};
        if let Some(key) = event::read()?.as_key_press_event() {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Char('j') | KeyCode::Down => self.counter -= 1,
                KeyCode::Char('k') | KeyCode::Up => self.counter += 1,
                _ => {}
            }
        }
        Ok(())
    }
}
```

For larger apps, render via `impl Widget for &App` (so `frame.render_widget(&self.some_component, area)` composes), and consider the Elm/Component patterns in [07-async-and-architecture.md](07-async-and-architecture.md).

## Viewports: fullscreen, inline, fixed

```rust
use ratatui::{TerminalOptions, Viewport};

let mut terminal = ratatui::init_with_options(TerminalOptions {
    viewport: Viewport::Inline(10),   // 10-row UI below existing shell output
});
```

- `Viewport::Fullscreen` — default for `init`/`run` (alternate screen).
- `Viewport::Inline(height)` — the UI occupies `height` rows at the bottom of the *normal* screen, coexisting with shell scrollback. Perfect for progress UIs (cargo-style), REPLs, and agent CLIs.
- `Viewport::Fixed(rect)` — pin to an exact region.

With inline viewports, `terminal.insert_before(height, |buf: &mut Buffer| { ... })` inserts permanent lines *above* the live UI (scrolling them into history) — e.g. completed-download lines, finished log records:

```rust
terminal.insert_before(1, |buf| {
    use ratatui::widgets::Widget;
    ratatui::text::Line::from("✔ task finished").render(buf.area, buf);
})?;
```

Note `init_with_options` enables raw mode but **not** the alternate screen. On exit, call `ratatui::restore()`; for inline UIs you usually also want `terminal.clear()` or a final `insert_before` to leave a tidy record.

## Suspending the TUI (spawn $EDITOR, shell out)

```rust
ratatui::restore();                              // leave alt screen + raw mode
std::process::Command::new("vim").arg(path).status()?;
let mut terminal = ratatui::init();              // re-enter
terminal.clear()?;                               // force full repaint
```

## Performance notes

- Drawing at 60fps is usually fine, but don't draw when nothing changed (draw-on-event) — terminals and SSH links appreciate it.
- The expensive part is usually *your* state → widget construction (string formatting, syntax highlighting), not the diff. Cache derived data in your app state when it gets hot.
- Layout results are cached (`layout-cache` default feature) keyed by (area, constraints).
- Never `println!`/`eprintln!` while the TUI is active — it corrupts the display (raw mode doesn't translate `\n` to `\r\n`). Log to a file instead (see tracing recipe in 07), or use `Viewport::Inline` + `insert_before`.
