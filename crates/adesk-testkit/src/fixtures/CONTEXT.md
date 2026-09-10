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
- `Exec` arguments are space-joined and auto-quoted by `exec_arg` when empty or containing ASCII whitespace, `"` or `\`; unit tests prove the registry tokenizer returns them unchanged and no shell is involved.
- `write_raw` rejects absolute paths but does not resolve `..`: relative paths must stay inside the share root by caller discipline (`write_entry` rejects escaping stems before writing).
- Every process wait is deadline-bounded; `TestApp::exit` kills on deadline and returns the resulting (signal) status, so callers must assert `status.success()`.
- Integration tests of this package may also use `env!("CARGO_BIN_EXE_adesk-test-app")`.
- **No exec-path override.** `TestAppSpec::desktop_entry` and `TestApp::spawn` hard-code `helper_bin_path("adesk-test-app")` (`HELPER_APP`); no public constructor takes a caller-supplied program path, and `TestAppSpec::with_arg` only appends arguments. A downstream crate that needs its own fixture binary must compose the `DesktopEntryFixture` by hand (its `exec` field is public) and register it via `FixtureDir::write_entry` + `TestRuntimeConfig::with_fixture_dir`, then launch it over AGP instead of `TestApp::spawn`.

## Routing Table

Leaf module: `mod.rs`.
