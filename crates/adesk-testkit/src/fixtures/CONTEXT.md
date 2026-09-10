# fixtures — `.desktop` files, temp data dirs, helper process

## Intent

Everything a test needs to pretend an application is installed and launch it: `.desktop` fixtures written into a temp `XDG_DATA_DIRS` share root, and the `adesk-test-app` helper process that opens a real toplevel.

## API Surface

- `FixtureDir` — `new`, `path`, `search_dir` (the share root), `applications_dir`, `write_entry`, `write_raw`, `remove_entry`, `write_app`.
- `DesktopEntryFixture` — field-per-key model, builders, `to_desktop_file()`.
- `TestAppSpec` — `new`, builders (`with_title`, `with_size`, `with_fill`, `with_exit_after`, `with_arg`, `with_exec`), `app_id`, `title`, `exec`, `cli_args()` (single source of the helper CLI grammar), `desktop_entry()`.
- `TestApp` — `spawn`, `app_id`, `pid`, `is_running`, `wait_for_exit`, `exit`, `kill`.
- `helper_bin_path(name)` — resolves a helper binary next to the running test executable.

## Constraints

- `FixtureDir` is a *share* root; `adesk-app-registry` appends `/applications` to every search dir.
- Returned `AppId`s must equal the registry's own id rule (path relative to the applications dir, `.desktop` stripped, `/` → `.`).
- `Exec` arguments are space-joined and auto-quoted by `exec_arg` when empty or containing ASCII whitespace, `"` or `\`; unit tests prove the registry tokenizer returns them unchanged and no shell is involved.
- `write_raw` rejects absolute paths but does not resolve `..`: relative paths must stay inside the share root by caller discipline (`write_entry` rejects escaping stems before writing).
- Every process wait is deadline-bounded; `TestApp::exit` kills on deadline and returns the resulting (signal) status, so callers must assert `status.success()`.
- Integration tests of this package may also use `env!("CARGO_BIN_EXE_adesk-test-app")`.
- **Exec-path override.** `TestAppSpec::with_exec` sets the fixture program; `TestAppSpec::exec` reports it (`None` by default). Both program-resolution sites — `TestAppSpec::desktop_entry` (`mod.rs:504`) and `TestApp::spawn` (`mod.rs:554`) — fall back to `helper_bin_path("adesk-test-app")` (`HELPER_APP`, `mod.rs:68`) when the spec carries no program, so a downstream crate can run its own fixture binary as the program without hand-composing a `DesktopEntryFixture`. `TestAppSpec::with_arg` only appends helper arguments and never changes the program.
- **`TestAppSpec::title` is the window title.** It defaults to the app id and `with_title` overrides it; it is what `desktop_entry` writes as `Name`.

## Notes for Agents

- `TestAppSpec::cli_args` (`mod.rs:478`) is the single encoder of the helper CLI grammar; the matching decoder lives in the `adesk-test-app` binary (`src/bin/adesk-test-app.rs`), not in this module.
- `DesktopEntryFixture`'s `exec` field is public (`mod.rs:200`), so a test that needs a program/args shape `TestAppSpec` cannot express can still hand-compose an entry and write it via `FixtureDir::write_entry`/`write_raw`.

## Routing Table

Leaf module: `mod.rs`.
