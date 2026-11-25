# nujson — Plan

Terminal viewer for NDJSON files (see `SPEC.md`).

## Tech stack

| Concern | Crate | Why |
|---|---|---|
| TUI | `ratatui` (re-exported `crossterm` backend) | De-facto standard; crossterm is cross-platform (Linux/macOS/Windows) |
| JSON | `serde_json` (`preserve_order`) | Keep key order as in the file |
| jq-like | `jaq-core` + `jaq-std` + `jaq-json` | Pure-Rust jq clone, no C deps → easy Windows build |
| CLI | `clap` (derive) | `nujson file.ndjson [--no-color] [--filter EXPR]` |
| Big files | `memmap2` | Index line offsets, parse lazily |
| Errors | `anyhow` | |
| Tests | `insta` + ratatui `TestBackend` | Snapshot UI tests |

Highlighting: custom `serde_json::Value` → ratatui `Span`s walker (no `syntect`); "no color" = same walker with default style.

## Layout

```
┌ Lines (1,204 / 50,000) ───┐┌ Line 42 ─────────────────────┐
│   41 {"ts":"...","lvl"... ││ {                            │
│ ▶ 42 {"ts":"...","lvl"... ││   "ts": "2026-11-25T10:00",  │
│   43 {"ts":"...","lvl"... ││   "level": "error",          │
│ ✗ 44 <invalid json>       ││   "msg": "boom"              │
└───────────────────────────┘└──────────────────────────────┘
 / search   | jq filter   c color   Tab focus   ? help   q quit
```

## Architecture

```
src/
├── main.rs          # CLI, terminal setup/teardown, panic hook
├── app.rs           # State: selection, focus, scroll, active filter, mode
├── source.rs        # mmap + line index (Vec<u64> offsets), get_line(i), parse(i)
├── event.rs         # Key → Action
├── render/pretty.rs # Value → Vec<Line> (colored/plain), Theme
├── filter/          # text.rs (smart-case substring), jq.rs (jaq, truthy = keep)
└── ui/              # list, detail, input prompt, status/help
```

- A filter produces `Vec<usize>` of matching line indices; the list shows the original line numbers.
- Filtering runs on a background thread with progress + cancellation.
- Invalid JSON lines are marked `✗`; detail pane shows raw text + parse error.
- jq semantics: keep a line if the expression yields any value other than `false`/`null` (and no error).

## Keybindings

`↑/↓ j/k` move · `PgUp/PgDn` · `g/G` top/bottom · `Tab` focus · `/` text search · `|` jq filter · `n/N` next/prev · `Esc` clear · `c` color · `w` wrap · `?` help · `q` quit

## Milestones

- [x] **M1 Skeleton** — cargo project, clap, terminal setup/restore (incl. panic), CI matrix (ubuntu/macos/windows: fmt, clippy, test).
- [ ] **M2 Viewer** — mmap + line index, left list, right raw view, navigation & scrolling.
- [ ] **M3 Pretty + highlight** — renderer, theme, `c` toggle, `--no-color`, `NO_COLOR` env.
- [ ] **M4 Text search/filter** — input prompt, smart-case substring, match count, `n/N`.
- [ ] **M5 jq filter** — jaq, compile errors in prompt, background eval with progress.
- [ ] **M6 Polish** — help popup, match highlighting in detail, stdin input, release binaries (`cargo-dist`).
- [ ] **M7 Later** — follow mode (`-f`), copy to clipboard, jq projection of detail view, config file.

## Testing

- Unit: line indexer (CRLF, no trailing newline, empty lines), pretty renderer, filters.
- Snapshots: full-frame renders via `TestBackend` + `insta`.
- Fixtures in `tests/fixtures/`, plus a generated ~1M-line file for perf checks.
