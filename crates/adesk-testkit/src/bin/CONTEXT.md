# bin — `adesk-test-app` helper binary

## Intent

A minimal Wayland client process used by fixture/launch tests: it connects to `$WAYLAND_DISPLAY`, opens a toplevel filled with a known pattern, and exits on command (stdin `exit` or `--exit-after`).
It is a test helper, never shipped; it links the testkit lib only to reuse `FillPattern`/`Size`.

## API Surface

CLI: `adesk-test-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] [--fill <PATTERN>] [--exit-after <MS>]`; `--fill` uses the `FillPattern::from_cli_arg` grammar.
Exit codes: 0 clean, 2 connect/protocol failure, 64 usage error.

## Constraints

- Assumes `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are set by the spawning test/registry.
- No clap: arguments are parsed by hand into `CliArgs`.
- Must never require a display, GPU or installed application.

## Routing Table

Leaf: `adesk-test-app.rs`.
