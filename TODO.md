# TODO / future work

Deferred ideas captured so the MVP stays small but nothing gets lost. See
`DESIGN.md` for the authoritative design and `CLAUDE.md` for conventions.

## Side rail (left gutter)

A vertical status rail down the left edge of the document view, aligned to each
runnable block, to make run state legible at a glance.

- **Per-block status, by color** — distinct colors/glyphs for each state:
  - `blocked` — earlier cell must run / gate not satisfied
  - `next` — the cell that would run next
  - `running`
  - `done` (success)
  - `error`
- **Run-order number** — when a cell has been run, show a small ordinal (1, 2,
  3, …) in the rail marking the order cells were executed. Absent until the cell
  runs.
- Only runnable cells (code; maybe input) get a rail marker; prose does not.
- Open question: how the rail interacts with the existing full-width selection
  highlight bar (layer the rail on top? reserve a left column outside it?).

## Code block output (Claude Code–style)

- **Truncated by default** — show at most ~5–10 lines of a cell's output.
- **Verbose toggle** — a key/mode to expand a cell's full output on demand.
- ~~**Run metadata on the cell** — elapsed run time + exit code.~~ **Built.**
  A finished cell's status line shows `✔ ok · 1.2s` (elapsed always) and appends
  `· exit N` only when the process exited non-zero (`✗ error · 0.4s · exit 2`).
  Timing lives on `CodeBlock` (`started_at`/`elapsed`), the exit code threads
  through `RunMsg::Finished { code }`. A *live* ticking timer for a running cell is
  deliberately the footer's job (it redraws every frame), not the cell's — updating
  it in the cached doc would force a per-frame re-wrap.
- Ties into the streaming/ANSI handling still under review in `DESIGN.md` §7.

## Notes

- These are explicitly post-MVP polish.
- **Cell execution + streaming: built.** Enter/`r` on a runnable shell cell
  (`sh`/`bash`/`zsh`, not `skip=true`) runs it as its own process off-thread.
  Output streams back line-by-line via `runner::run_streaming` over a `RunMsg`
  channel arm in the draw loop (`Output` chunks then one `Finished`); each chunk
  appends to `CodeBlock.output` and bumps `revision`. The env map
  (`Runbook::env_for`) layers frontmatter `env` → `TMP_DIR` → preceding answered
  input cells (DESIGN §4); `before_each`/`after_each` and `interpreters.<lang>.path`
  remap are honored. `TMP_DIR` is a real `mktemp` dir, cleaned on drop unless
  `skip_cleanup`. Output renders under the cell as the *tail* (last
  `OUTPUT_MAX_LINES` = 10 lines, "… N earlier lines" marker above). Status
  (`CodeBlockState`) and the output buffer are separate fields, per the three-tier
  note. `runner::run_script` (collect-all) is kept for the non-TUI `exec` path.
  Still deferred:
  - **ANSI / true stdout-stderr ordering** (DESIGN §7) — streams are merged by
    arrival, not strictly ordered (needs a pty); ANSI escapes are passed through
    raw, not interpreted.
  - **Animated spinner while running** — needs a draw-time overlay (the running
    status line is in the cached doc, so it can't animate without a per-frame
    `revision` bump). Lands with the overlay/`DocLayout` work.
  - ~~**Run metadata** — elapsed time + exit code on the cell.~~ **Built** — see
    the "Code block output" section above.
  - **Verbose toggle** to expand past the 10-line output tail.

- **Footer / status bar: wired to real run state.** The bottom bar's badge now
  reflects an aggregate derived fresh each frame from `Runbook::run_counts()`:
  `running` (any cell executing) → `error` (any current failure) → `done` (any
  success) → `ready`. The braille **spinner lives inside the badge**, left of the
  text, and only animates while running; otherwise the badge shows a static
  state glyph (`◦`/`✔`/`✗`). The middle shows mode-aware key hints (navigate vs.
  edit); the right shows `N/M complete` run progress. The badge tracks run state
  only — input-editing mode is conveyed by the cell highlight, not the footer.
  Still to polish:
  - **Tune the badge colors.** `fg` is currently black on every status; black-on-red
    (error) reads muddy and black-on-blue (ready) is low-contrast on some themes.
    Pick per-status fg/bg pairs with decent contrast (e.g. white-on-red for error),
    ideally against both light and dark terminal backgrounds.

  Note on the **`DocLayout` refactor below**: streaming did *not* require it. The
  draw loop rebuilds the wrapped-line cache lazily — at most once per frame — so
  many `Output` chunks between frames coalesce into a single whole-doc re-wrap at
  30fps, not one per line. The refactor is now a **large-document** perf
  optimization (and the enabler for the animated spinner overlay), not a
  prerequisite for streaming.
- **Input blocks: widget + state are built** (confirm / input / select, with an
  Enter-to-edit `Mode::Active` focus model and an answered `resolved() ->
  (target, value)` seam, now wired into `env_for`). Still deferred:
  - **`option_file` / `$TMP_DIR` resolution** for select cells (currently only
    inline `options` render; `option_file` is parsed but not read). *Now
    unblocked* — `TMP_DIR` exists, so a preceding cell can produce the file.
  - **Multi-select**, and a **real terminal cursor** for the text field (today's
    caret is a synthetic reversed cell, fine for the flat-line scrollview).

## Decisions

### Per-block layout model (deferred — now a large-doc perf optimization)

Today the document is rendered as one flat `Vec<Line>` cached by `width +
revision`, with a `ranges: Vec<Range<usize>>` sidecar mapping block → line span.
Any content change bumps `revision` and re-wraps the *whole* document. This held
up fine through execution + streaming output: because the cache rebuilds lazily
(once per frame at most), a burst of streamed chunks coalesces into a single
re-wrap per frame, not one per line. So the trigger for this refactor is no
longer "streaming lands" — it's **a document large enough that one whole-doc
re-wrap per frame is visibly costly**, or wanting the **draw-time overlay** that
the animated running-spinner needs.

**Decision:** when we build execution + output blocks, move to a retained
per-block layout:

```rust
struct DocLayout { width: u16, blocks: Vec<BlockLayout> }
struct BlockLayout { height: u16, lines: Vec<Line<'static>>, dirty: bool }
```

- Scrolling runs off `height` (prefix-sum); selection range = sum up to block.
- **Localized invalidation is the payoff:** a changed block re-wraps alone;
  others keep cached `lines` and only their y-offset shifts (re-sum heights).
  Update cost goes O(whole-doc re-wrap) → O(one block) + O(blocks) re-sum.
- A width change still triggers a full rebuild (rare; fine).
- **Lazy/windowed materialization only for output blocks** (which can be huge /
  streaming): cache height, materialize just the visible slice. Prose/code/input
  materialize eagerly — full laziness buys little there. Output is also capped to
  ~5–10 lines unless verbose, which sidesteps most of it.
- Watch scroll-offset stability when a block *above* the viewport grows: anchor
  scroll to a block + intra-block offset (or tail-follow only at bottom) rather
  than a raw global line index, so the view doesn't jump.

### Three tiers + where state lives

Keep three layers distinct:

1. **Source / parse** — `mdast` → `Runbook.blocks`. Built once at load,
   immutable for the session (until live reload exists).
2. **Cell / selectable** — the navigable units plus their state.
3. **Layout / physical lines** — wrapped `Vec<Line>` per block, derived from
   1 + 2 at a given width (the `DocLayout` above).

Tier-2 state is **not** monolithic. Split it by (a) does it change the block's
rendered lines, and (b) how fast does it change:

| State | Changes lines? | Frequency | Lives in |
|---|---|---|---|
| selected (highlight) | no — overlay | every `j/k` | view state |
| active / focused | no (ring); yes (draft) | rare toggle | view state |
| status idle/run/ok/err | yes | few per run | model (on block) |
| streamed output | yes (appends) | **fast** | model (on block) |
| draft text (editing) | yes | **fast** | model-ish |
| expanded / collapsed | yes (height) | rare toggle | view state |

**Design rules:**

- **Decoration that changes often but is cheap** (selection highlight, rail
  marker, focus ring) → draw-time overlay, **never invalidates layout**. Already
  true: selection lives in `ScrollState`, not the cache key, so `j/k` re-wraps
  nothing. Keep it that way.
- **Content that changes fast** (output, draft) → per-block dirty, so a fast
  update re-wraps exactly one block.
- **Model state vs view state:** status, output buffer, answered value belong to
  the *document* → store on the block (like `CodeBlock.state`). Selection, scroll
  offset, focus, expanded-set are the *UI's* opinion → store in
  `ScrollState`/`App`. Model state is the document; view state is throwaway.
- **The one deliberate exception:** `expanded` is view state yet changes height,
  so it must mark the block's layout dirty. Rare, so it's free — just conscious
  that "view state never touches layout" has that single hole.

Net: nothing is both fast *and* expensive, as long as decoration stays a
draw-time overlay and the two fast content streams (output, draft) route through
per-block invalidation.
