# nujson

Terminal viewer for NDJSON (newline-delimited JSON) files: a list of lines on the
left, the selected line pretty-printed and highlighted on the right, with text and
jq filters. Runs on Linux, macOS and Windows.

```
┌ app.ndjson · 1 / 5 ───────────────┐┌ Line 3 ──────────────────────────┐
│▶ 3 {"ts":"2026-11-25T10:00:02Z",…  ││{                                 │
│                                   ││  "level": "error",               │
│                                   ││  "msg": "connection refused",    │
│                                   ││  "code": 503                     │
└ |.code >= 500 ────────────────────┘└──────────────────────────────────┘
```

## Usage

```sh
nujson app.ndjson
nujson app.ndjson -f '.level == "error"'   # start with a jq filter
kubectl logs my-pod | nujson                # read from stdin
nujson --no-color app.ndjson                # or set NO_COLOR=1
```

Large files are memory-mapped and filters run on all cores in the background.
Lines that are not valid JSON are marked with `✗`.

## Keys

| Key | Action |
|---|---|
| `↑↓` `j` `k` | move selection / scroll detail |
| `PgUp` `PgDn` `Space` | page |
| `g` `G` `Home` `End` | first / last |
| `Tab` | switch pane (`Enter`/`l` → detail, `h` → list) |
| `←` `→` | scroll detail horizontally (wrap off) |
| `/` | text filter: case-insensitive unless the query has an uppercase letter; matches are highlighted |
| `\|` | jq filter: keeps lines where the expression yields a value other than `false`/`null` |
| `Esc` | cancel a running filter, then clear the filter |
| `w` | toggle line wrap |
| `c` | toggle JSON colors |
| `?` | help |
| `q` `Ctrl-C` | quit |

jq filter examples: `.level == "error"`, `.code >= 500`, `select(.user.id == 42)`,
`.msg | test("^conn")`, `has("trace_id")`. The jq dialect is [jaq](https://github.com/01mf02/jaq).

## Build

```sh
cargo build --release   # binary in target/release/nujson
cargo test
```

Pushing a `v*` tag builds release archives for Linux (x86_64/aarch64, static
musl), macOS (x86_64/arm64) and Windows (x86_64) via GitHub Actions.
