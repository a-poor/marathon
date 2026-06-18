# Events and Input

Ratatui has **no event system** — input comes from the backend, almost always crossterm. This file covers the crossterm event model, keyboard/mouse/paste/resize handling, and loop integration. Targets ratatui 0.30 / crossterm 0.29.

Import from your own `crossterm = "0.29"` dependency or from the re-export `ratatui::crossterm` (identical, version-safe).

## The event model

```rust
use crossterm::event::{self, Event};

let event: Event = event::read()?;        // blocking
if event::poll(Duration::from_millis(100))? {   // wait up to timeout for availability
    let event = event::read()?;                  // now guaranteed not to block
}
```

```rust
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),       // only when mouse capture is enabled
    Paste(String),           // only when bracketed paste is enabled
    Resize(u16, u16),
    FocusGained, FocusLost,  // only when focus-change reporting is enabled
}
```

Crossterm 0.29 convenience helpers (used throughout ratatui's examples):

```rust
event.is_key_press();                            // bool
event.as_key_press_event();                      // Option<KeyEvent> — filters kind == Press
```

## Keyboard

```rust
use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

if let Some(key) = event::read()?.as_key_press_event() {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => quit(),      // you MUST handle Ctrl+C in raw mode
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => insert(c),  // shifted chars arrive pre-shifted ('A')
        (KeyCode::Enter, _) => submit(),
        (KeyCode::Esc, _) => cancel(),
        (KeyCode::Backspace, _) | (KeyCode::Delete, _) => delete(),
        (KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down, _) => move_cursor(key.code),
        (KeyCode::Tab, _) | (KeyCode::BackTab, _) => cycle_focus(),  // BackTab = Shift+Tab
        (KeyCode::F(5), _) => refresh(),
        (KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown, _) => scroll(key.code),
        _ => {}
    }
}
```

**Always filter `KeyEventKind::Press`** (directly or via `as_key_press_event`). Windows delivers `Release` and `Repeat` events too; unfiltered apps act twice per keystroke on Windows. This is the #1 cross-platform bug.

Gotchas:

- `KeyCode::Char(c)` already includes shift effects (`'A'`, `'%'`); `key.modifiers` still has `SHIFT` set. Match `Char(c)` with `NONE | SHIFT` modifiers when inserting text, so you don't swallow Ctrl/Alt chords.
- In a plain terminal you **cannot** distinguish `Shift+Enter`/`Ctrl+Enter` from `Enter`, or `Tab` from `Ctrl+I` — the legacy protocol doesn't encode them. See *kitty keyboard protocol* below; this matters for chat inputs (06).
- `Alt+x` arrives as `Char('x')` with `KeyModifiers::ALT` — Alt-chords are reliably detectable and a good fallback for "insert newline" etc.
- Some keys are intercepted by terminals/OS (e.g. `Ctrl+Tab`, function keys under tmux configs). Offer alternatives.

### Mode-based dispatch (vim-ish, or "typing vs navigating")

```rust
enum Mode { Normal, Editing }

match app.mode {
    Mode::Normal => match key.code {
        KeyCode::Char('i') => app.mode = Mode::Editing,
        KeyCode::Char('q') => app.quit(),
        _ => {}
    },
    Mode::Editing => match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Char(c) => app.input.insert_char(c),   // chars are text now, not hotkeys
        _ => {}
    },
}
```

The crucial idea: **when an input is focused, printable chars go to the input, not to hotkeys.** Reserve only non-printable keys (Esc, Enter, Tab, Ctrl-chords) as commands in editing mode.

## Mouse

Mouse capture is opt-in, *in addition to* `ratatui::init()`/`run()`:

```rust
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, MouseEvent, MouseEventKind, MouseButton};
use crossterm::execute;

ratatui::run(|terminal| {
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = app.run(terminal);
    execute!(std::io::stdout(), DisableMouseCapture)?;   // before returning
    result
})
```

```rust
Event::Mouse(mouse) => match mouse.kind {
    MouseEventKind::Down(MouseButton::Left) => {
        let pos = ratatui::layout::Position::new(mouse.column, mouse.row);
        if app.sidebar_area.contains(pos) { app.click_sidebar(mouse.row); }
    }
    MouseEventKind::Drag(MouseButton::Left) => app.drag_to(mouse.column, mouse.row),
    MouseEventKind::ScrollUp => app.scroll = app.scroll.saturating_sub(3),
    MouseEventKind::ScrollDown => app.scroll = (app.scroll + 3).min(app.max_scroll()),
    MouseEventKind::Moved => app.hover(mouse.column, mouse.row),
    _ => {}
}
```

Hit-testing pattern: store each clickable widget's `Rect` in app state during `render` (they're just values), then `rect.contains(position)` in the handler. Caveat: with mouse capture on, the terminal's native text selection/copy stops working (users must hold Shift in most terminals) — enable mouse only if you need it.

## Bracketed paste

Without this, a paste arrives as a storm of individual `Key` events (including `Enter`s — dangerous in a chat input!).

```rust
use crossterm::event::{EnableBracketedPaste, DisableBracketedPaste};
execute!(std::io::stdout(), EnableBracketedPaste)?;
// ...
Event::Paste(s) => app.input.insert_str(&s),   // one event, whole pasted text (may contain \n)
```

Enable it for any app with a text input. Disable on exit (same pattern as mouse capture).

## Focus events

```rust
use crossterm::event::{EnableFocusChange, DisableFocusChange};
// Event::FocusGained / Event::FocusLost
```

Use to pause animations/polling when the terminal loses focus, or dim the UI.

## Resize

`Event::Resize(cols, rows)` — usually nothing to do beyond redrawing (the next `terminal.draw` re-measures). If you cache layout-dependent state (scroll offsets, wrapped text), invalidate it here.

## Kitty keyboard protocol (keyboard enhancement flags)

Modern terminals (kitty, foot, WezTerm, Ghostty, recent iTerm2/alacritty…) support an enhanced protocol that disambiguates keys the legacy protocol can't: `Shift+Enter`, `Ctrl+Enter`, `Esc` vs `Alt+...`, key release events, etc.

```rust
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::supports_keyboard_enhancement;
use crossterm::execute;

let enhanced = supports_keyboard_enhancement().unwrap_or(false);
if enhanced {
    execute!(std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        ))?;
}
// ... app runs; Shift+Enter now arrives as KeyCode::Enter + KeyModifiers::SHIFT ...
if enhanced {
    execute!(std::io::stdout(), PopKeyboardEnhancementFlags)?;
}
```

Always **feature-detect and keep a fallback** (e.g. Alt+Enter) — plenty of terminals still lack support. See 06 for the chat-input application of this.

## Loop integration patterns

### A. Block on `read()` — redraw per event

Zero idle CPU. Right default for non-animated apps. (Skeleton in 01-core-concepts.md.)

### B. `poll(timeout)` tick loop — animations, background polling

(Skeleton in 01-core-concepts.md.) Drain bursts before redrawing:

```rust
while event::poll(Duration::ZERO)? {
    handle(event::read()?);
}
```

### C. Dedicated input thread + channel — uniform "message" handling

Decouples input from rendering; lets background jobs send into the same channel:

```rust
use std::sync::mpsc;
use std::time::{Duration, Instant};

enum AppEvent {
    Input(crossterm::event::Event),
    Tick,
    JobDone(JobResult),
}

fn spawn_event_thread(tx: mpsc::Sender<AppEvent>, tick: Duration) {
    std::thread::spawn(move || {
        let mut last = Instant::now();
        loop {
            let timeout = tick.saturating_sub(last.elapsed());
            if crossterm::event::poll(timeout).unwrap_or(false) {
                if let Ok(ev) = crossterm::event::read() {
                    if tx.send(AppEvent::Input(ev)).is_err() { break; }
                }
            }
            if last.elapsed() >= tick {
                if tx.send(AppEvent::Tick).is_err() { break; }
                last = Instant::now();
            }
        }
    });
}

// main loop:
// for event in rx { match event { AppEvent::Input(e) => .., AppEvent::Tick => draw, .. } }
```

### D. Async `EventStream` (tokio) — see 07-async-and-architecture.md

```rust
// crossterm features = ["event-stream"], plus tokio + futures/tokio-stream
let mut events = crossterm::event::EventStream::new();
tokio::select! {
    Some(Ok(event)) = events.next() => app.handle_event(&event),
    _ = redraw_interval.tick() => { terminal.draw(|f| app.render(f))?; }
    Some(msg) = rx.recv() => app.handle_message(msg),
}
```

## Translating events to actions (recommended indirection)

Map raw events to a domain `Action`/`Message` enum early; the rest of the app never sees crossterm types. This enables keymaps, vim/emacs profiles, testing without a terminal, and async message mixing:

```rust
enum Action { Quit, MoveUp, MoveDown, Submit, Insert(char), Paste(String) }

fn to_action(event: &Event, mode: &Mode) -> Option<Action> { /* match ... */ }

// update(state, action) is then a pure, unit-testable function (Elm-style; see 07).
```
