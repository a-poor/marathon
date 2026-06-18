# Async, Background Work, and App Architecture

Keeping the UI responsive while data arrives, and organizing code as apps grow. Targets ratatui 0.30.

## Do you need async?

Ratatui itself is sync and doesn't care. Decision guide:

- **No background work** → blocking `event::read()` loop. Done.
- **A few fire-and-forget jobs** (file scan, HTTP fetch, LLM stream) → `std::thread::spawn` + `mpsc` channel + `event::poll` tick loop. No tokio needed; this is underrated.
- **Many concurrent IO tasks, streaming APIs, existing async client libraries** → tokio + `EventStream`.

## Pattern 1: Threads + channels (std-only)

One channel carries *everything*; the main loop is a simple message pump:

```rust
use std::sync::mpsc;

enum AppEvent {
    Terminal(crossterm::event::Event),
    SearchResults(Vec<Row>),
    DownloadProgress { id: u64, pct: f64 },
    Tick,
}

fn main() -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();

    // input thread
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                if tx.send(AppEvent::Terminal(ev)).is_err() { break; }
            }
        });
    }
    // tick thread (only if you animate)
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if tx.send(AppEvent::Tick).is_err() { break; }
        });
    }

    ratatui::run(|terminal| {
        let mut app = App::default();
        while !app.quit {
            terminal.draw(|frame| app.render(frame))?;
            // Block on the first event, then drain the rest before redrawing:
            let Ok(first) = rx.recv() else { break };
            app.update(first, &tx);
            while let Ok(more) = rx.try_recv() {
                app.update(more, &tx);
            }
        }
        Ok(())
    })
}
```

Workers get a `tx.clone()` and send results as messages (`app.update` spawns them). Draining with `try_recv` coalesces bursts (stream tokens, progress spam) into one redraw.

## Pattern 2: Tokio + `EventStream`

```toml
tokio = { version = "1", features = ["full"] }
crossterm = { version = "0.29", features = ["event-stream"] }
tokio-stream = "0.1"     # or futures = "0.3", for StreamExt
```

The canonical loop (this is ratatui's `async-github` example shape):

```rust
use crossterm::event::{Event, EventStream};
use tokio_stream::StreamExt;
use std::time::Duration;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();          // init/restore around the async body
    let result = App::default().run(terminal).await;
    ratatui::restore();
    result
}

impl App {
    async fn run(mut self, mut terminal: ratatui::DefaultTerminal) -> color_eyre::Result<()> {
        let mut events = EventStream::new();
        let mut frames = tokio::time::interval(Duration::from_secs_f32(1.0 / 30.0));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppMsg>();

        self.spawn_initial_tasks(tx.clone());

        while !self.quit {
            tokio::select! {
                _ = frames.tick() => { terminal.draw(|frame| self.render(frame))?; }
                Some(Ok(event)) = events.next() => self.handle_terminal_event(&event, &tx),
                Some(msg) = rx.recv() => self.handle_msg(msg),
            }
        }
        Ok(())
    }
}
```

Notes:

- Drawing on a fixed interval (30–60fps) is the simplest correct policy under async; if you want draw-on-change instead, keep a `dirty: bool` and draw only when set.
- Spawn work with `tokio::spawn`; send results over the channel — **don't** mutate UI state from tasks. Alternative (used by `async-github`): share `Arc<RwLock<WidgetState>>` between a task and the render path; fine for "one widget = one fetcher" designs, channels scale better.
- Use `tokio::task::spawn_blocking` for CPU-heavy or blocking work.
- `ratatui::run` works with async via `ratatui::run(|t| tokio_runtime.block_on(app.run(t)))`, but explicit `init`/`restore` around the async body (as above) is clearer.

### Streaming an LLM response (async version of 06 §5)

```rust
fn send_prompt(&mut self, prompt: String, tx: tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    self.messages.push(Msg::assistant_empty());
    tokio::spawn(async move {
        // e.g. reqwest SSE / anthropic / openai client yielding chunks:
        let mut stream = client.stream_completion(prompt).await?;
        while let Some(chunk) = stream.next().await {
            if tx.send(AppMsg::Token(chunk?)).is_err() { break; }   // UI gone -> stop
        }
        let _ = tx.send(AppMsg::StreamDone);
        Ok::<_, anyhow::Error>(())
    });
}
// handle_msg: AppMsg::Token(t) => self.messages.last_mut().unwrap().content.push_str(&t)
```

Cancellation: keep the `JoinHandle` (or a `CancellationToken`) so Esc can abort a generation; the dropped/closed channel stops the task at the next send.

## Architectures

For small apps, the `App { state..., render(), handle_events() }` struct from 01 is all you need. As apps grow, two patterns dominate (write-ups: https://ratatui.rs/concepts/application-patterns/):

### The Elm Architecture (TEA) — Model / Message / Update / View

```rust
enum Message { Quit, NextTab, Select(usize), Token(String), /* ... */ }

fn handle_event(model: &Model, ev: &Event) -> Option<Message> { /* translate only */ }
fn update(model: &mut Model, msg: Message) -> Option<Message> { /* ALL mutation here; may chain */ }
fn view(model: &Model, frame: &mut Frame) { /* render only, no mutation */ }

// loop: draw(view) -> event -> handle_event -> update (follow chained messages) -> repeat
```

Strengths: every state change is an enumerable, loggable, *unit-testable* value (`update` needs no terminal); async tasks just send `Message`s into the same funnel. This is the pattern that scales best with LLM/agent apps where many things mutate state.

### Component architecture

Each screen/panel is a struct implementing a common trait, owning its state and child components:

```rust
trait Component {
    fn handle_event(&mut self, ev: &Event) -> Option<Action>;   // bubble actions up
    fn update(&mut self, action: &Action);                       // react to global actions
    fn render(&self, frame: &mut Frame, area: Rect);
}
```

A root `App` routes events to the focused component, broadcasts `Action`s, and lays out children. The `ratatui/templates` component template (`cargo generate ratatui/templates component`) ships this wiring complete with config, keybinding maps, and tracing.

Hybrid is normal: Elm-style message funnel at the top, component structs for rendering.

### Multi-screen apps

```rust
enum Screen { Dashboard(DashboardState), Detail(DetailState), Help }
// match on &mut self.screen for both event-routing and rendering; push/pop a Vec<Screen> for modal stacks
```

## Code organization (the conventional split)

```
src/
  main.rs        # terminal setup, top-level loop
  app.rs         # App/Model state + update logic
  event.rs       # event reader thread/stream -> AppEvent
  ui.rs          # view: fn render(app, frame) (or ui/ with one module per panel)
  action.rs      # Message/Action enums
  components/    # component structs (if using that pattern)
```

Render from `&App` (`impl Widget for &App` is a tidy root: `frame.render_widget(&app, frame.area())`).

## Testing

### Unit-test update logic (no terminal needed)

The payoff of TEA: `update(&mut model, Message::Select(3)); assert_eq!(model.selected, Some(3));`

### Render tests with `TestBackend`

```rust
use ratatui::{backend::TestBackend, Terminal};

#[test]
fn renders_title() {
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let app = App::with_items(vec!["a", "b"]);
    terminal.draw(|frame| app.render(frame)).unwrap();
    // TestBackend renders to an in-memory Buffer (its Error = Infallible in 0.30):
    insta::assert_snapshot!(terminal.backend());   // snapshot the screen as text
}
```

`TestBackend` implements `Display`, so `insta` snapshot tests give you reviewable "screenshots" (`cargo insta review`). You can also assert on specific cells via `terminal.backend().buffer()`. Drive interactions by calling your `update`/`handle_event` functions directly with synthetic `KeyEvent`s — input doesn't go through the backend.

### Widget-level tests

```rust
let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 20, 3));
(&MyWidget { /* ... */ }).render(buf.area, &mut buf);
assert_eq!(buf, Buffer::with_lines(["expected line 1     ", /* ... */]));
```

## Logging & debugging

You can't print to the screen you're drawing on. Options:

- `tracing` + `tracing-appender` file logs, `tail -f` in another pane (recipe: https://ratatui.rs/recipes/apps/log-with-tracing/); `tui-logger` shows logs *inside* the app.
- `dbg!`/`eprintln!` work if you redirect stderr: `cargo run 2>debug.log`.
- Panics: 0.30's `init` already restores the terminal on panic, so backtraces print readably; add `color-eyre` for pretty ones (install hook *before* `ratatui::init`).

## Performance checklist

1. Don't redraw when nothing changed (event-driven loop, or `dirty` flag under async).
2. Coalesce message bursts before redrawing (drain the channel).
3. Build text/widgets from borrowed `&str` where possible (`Span`/`Line` are `Cow`-based); avoid `clone()`ing big strings per frame.
4. Cache expensive derivations (syntax highlighting, wrapped layouts, filtered lists) keyed by (input, width) and invalidate on change/resize.
5. Virtualize huge lists: render only the visible slice yourself (`List` already skips offscreen items; for 100k+ rows slice your data before building items).
6. Profile the draw closure first — it's almost always string formatting, not ratatui's diff.
