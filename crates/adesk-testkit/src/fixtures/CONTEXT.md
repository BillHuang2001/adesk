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

## Notes for Agents

- No exec-path override exists: `TestAppSpec::desktop_entry` (`mod.rs:477`) and `TestApp::spawn` (`mod.rs:523`) both hardcode `helper_bin_path(HELPER_APP)` with `HELPER_APP = "adesk-test-app"` (`mod.rs:67`); `TestAppSpec` has no program-path setter and no env var overrides the program.
  The only reuse seam for a downstream crate that wants to point a fixture at its own binary is to bypass `TestAppSpec::desktop_entry`/`TestApp::spawn` and build a `DesktopEntryFixture` by hand (`exec` is a public field, `mod.rs:199`) written via `FixtureDir::write_entry`/`write_raw`.
- `TestAppSpec::cli_args` (`mod.rs:451`) is the single encoder of the helper CLI grammar; the matching decoder lives in the `adesk-test-app` binary (`src/bin/adesk-test-app.rs`), not in this module.

## Routing Table

Leaf module: `mod.rs`.
