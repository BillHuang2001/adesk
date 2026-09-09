# fixtures — `.desktop` files, temp data dirs, helper process

## Intent

Everything a test needs to pretend an application is installed and launch it: `.desktop` fixtures written into a temp `XDG_DATA_DIRS` share root, and the `adesk-test-app` helper process that opens a real toplevel.

## API Surface

- `FixtureDir` — `new`, `path`, `search_dir` (the share root), `applications_dir`, `write_entry`, `write_raw`, `remove_entry`, `write_app`.
- `DesktopEntryFixture` — field-per-key model, builders, `to_desktop_file()`.
- `TestAppSpec` — `new`, builders, `cli_args()` (single source of the helper CLI grammar), `desktop_entry()`.
- `TestApp` — `spawn`, `app_id`, `pid`, `is_running`, `wait_for_exit`, `exit`, `kill`.
- `helper_bin_path(name)` — resolves a helper binary next to the running test executable.

## Constraints

- `FixtureDir` is a *share* root; `adesk-app-registry` appends `/applications` to every search dir.
- Returned `AppId`s must equal the registry's own id rule (path relative to the applications dir, `.desktop` stripped, `/` → `.`).
- `.desktop` `Exec` args are space-joined: fixture arguments must not require shell quoting.
- Every process wait is deadline-bounded; `TestApp::exit` kills on deadline.
- Integration tests of this package may also use `env!("CARGO_BIN_EXE_adesk-test-app")`.

## Routing Table

Leaf module: `mod.rs`.
