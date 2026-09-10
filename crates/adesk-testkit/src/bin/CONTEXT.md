# bin — `adesk-test-app` helper binary

## Intent

A minimal Wayland client process used by fixture/launch tests: it connects to `$WAYLAND_DISPLAY`, opens a toplevel filled with a known pattern, and exits on command (stdin `exit`, `--exit-after`, `--exit-on-close`, or a lost connection).
It is a test helper, never shipped; it links the testkit lib only to reuse `FillPattern`/`Size`.

## API Surface

CLI: `adesk-test-app --app-id <ID> [--title <TITLE>] [--size <W>x<H>] [--fill <PATTERN>] [--exit-after <MS>] [--exit-on-close]`; `--fill` uses the `FillPattern::from_cli_arg` grammar, and `--exit-on-close` is a valueless boolean while every other flag takes exactly one value.
Exit codes: 0 clean, 2 connect/protocol failure, 64 usage error.

## Constraints

- Assumes `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are set by the spawning test/registry.
- No clap: arguments are parsed by hand into `CliArgs` (the value-taking `FLAGS` plus the valueless `BOOL_FLAGS`).
- `--exit-on-close` is opt-in: absent it, the helper ignores `xdg_toplevel.close` and keeps pumping (the default self-exit behaviour), so a fixture must ask for close-driven exit explicitly.
- Must never require a display, GPU or installed application.
- The CLI grammar is a contract with `adesk_testkit::TestAppSpec::cli_args`; the `#[cfg(test)]` unit tests cover defaults, ordering, last-occurrence-wins, `--exit-on-close`, usage errors and the `--size`/`--fill`/`--exit-after` parsers.

## Routing Table

Leaf: `adesk-test-app.rs`.
