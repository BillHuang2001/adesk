# adesk-app-registry — XDG application registry

## Intent

`adesk-app-registry` is the runtime's application sensor and actuator: it discovers XDG `.desktop` entries, describes them to the agent, launches processes, and correlates launched processes with the windows they create.
It implements the registry half of `docs/architecture.md` §7 and AGP §5.2 (`list_apps`, `get_app`, `launch_app`); `docs/protocol.md` and `docs/architecture.md` are binding.
Sync only: no tokio, no async, no Smithay.
The only I/O is filesystem reads during `scan()` and process spawn during `launch()`; both are behind injectable seams (`ProcessSpawner`, `Clock`) so tests never touch the real environment.
`adesk-core` owns `AppId`, `AppInfo`, `LaunchId`, `WindowId` and `ErrorCode` — this crate depends on them and never forks them.

## API Surface

Every item below exists in `src/` and is re-exported flat at the crate root; the modules are `pub` as well.

### Registry (`src/registry.rs`)
- `RegistryOptions { search_dirs: Vec<PathBuf>, terminal: TerminalSpec, spawner: Arc<dyn ProcessSpawner>, clock: Arc<dyn Clock>, locale: Option<String> }` + `xdg()`, `with_search_dirs(iter)`, `with_spawner`, `with_clock`, `with_terminal`, `with_locale`.
- `AppRegistry`: `new()`, `with_options(RegistryOptions)`, `options()`, `search_dirs() -> &[PathBuf]`, `scan() -> Result<ScanReport>`, `list(query: Option<&str>, include_hidden: bool) -> Vec<AppInfo>`, `get(&AppId) -> Option<AppInfo>`, `contains(&AppId) -> bool`, `launch(&AppId, args: &[String], env: &LaunchEnv) -> Result<LaunchRecord>`, `len()`, `is_empty()`.
- `AppRegistry` is `Send + Sync`; methods take `&self` and the entry table lives behind an internal `RwLock`, so the server shares it as `Arc<AppRegistry>`.
- `ScanReport { dirs_scanned, files_scanned, apps, skipped, issues: Vec<ScanIssue> }`, `ScanIssue { path, reason }`.

### Launch seam (`src/launch.rs`)
- `LaunchRecord { launch_id: LaunchId, app_id: AppId, pid: Option<i32>, started_at_ms: u64 }`.
- `LaunchEnv { wayland_display: Option<String>, xdg_runtime_dir: Option<String>, extra: Vec<(String, String)> }` + `new`, `with_wayland_display`, `with_xdg_runtime_dir`, `with_var`, `overrides() -> Vec<(String, String)>`.
- `SpawnCommand { program, args, env }` + `new`, `with_arg`, `with_env`; `SpawnedProcess { pid }` + `new`.
- `trait ProcessSpawner: Send + Sync + Debug { fn spawn(&self, &SpawnCommand) -> Result<SpawnedProcess, SpawnError> }`; `CommandSpawner` is the production `std::process::Command` implementation.
- `SpawnError::{Io { program, source }, Other(String)}`.

### Exec expansion (`src/exec.rs`)
- `ExecExpander::{new, tokenize, expand_tokens, expand}`; `ExecContext { name, icon, desktop_file, files }`; `ExecError::UnterminatedQuote { offset }`.

### Terminal (`src/terminal.rs`)
- `TerminalSpec::{new, with_args, from_env, from_env_value, program, prefix_args, wrap}`; consts `DEFAULT_TERMINAL = "x-terminal-emulator"`, `TERMINAL_ENV = "TERMINAL"`, `EXEC_SEPARATOR = "-e"`.

### Correlation (`src/correlate.rs`)
- `Correlator::{new, with_timeout, record_launch, correlate, pending}`; `WindowCandidate { window_id, pid, app_id, title }`; `CorrelationOutcome::{Correlated(Correlation), Uncorrelated}`; `CorrelationEvidence::{Pid, StartupWmClass, AppIdOrTitleSubstring}`; `DEFAULT_CORRELATION_TIMEOUT = 10s`.

### Parsing, ids, XDG, TryExec, clock, errors
- `parse_str(&str) -> Result<RawEntry, ParseError>`; `RawEntry::{get, localized}`; `DesktopEntry::{from_raw, is_listable, to_app_info}`; `EntryError::{MissingType, NotApplication(String), MissingName}`; `ParseError::MissingGroup`.
- `desktop_file_id(&Path, &Path) -> Option<AppId>`, `is_desktop_file(&Path) -> bool`.
- `search_dirs()`, `search_dirs_with(data_home: Option<&Path>, data_dirs: Option<&str>, home: Option<&Path>)`.
- `resolve_try_exec(&str)`, `resolve_try_exec_with(&str, Option<&str>)`, `is_executable(&Path)`.
- `trait Clock: Send + Sync + Debug { fn now_ms(&self) -> u64 }`, `MonotonicClock`.
- `Error` + `code() -> ErrorCode` + `From<Error> for adesk_core::Error`; `pub type Result<T>`.

## Constraints

- Sync only; no tokio, no async, no Smithay.
- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`; no panics on request paths.
- Dependencies: `adesk-core`, `thiserror`, `tracing` (dev: `tempfile`); versions only from the root `[workspace.dependencies]`.
- Files stay under the ~1000-line threshold (largest: `src/registry.rs`, 580 lines); split along module boundaries rather than growing a file.
- Public API is what this file documents; keep internals `pub(crate)`.

## Routing Table

| Area | Owner |
|---|---|
| Registry state, scan/list/get/launch pipeline, options | `./src/registry.rs` |
| Crate error and AGP `ErrorCode` mapping | `./src/error.rs` |
| `LaunchRecord`, `LaunchEnv`, `SpawnCommand`, `ProcessSpawner`, `CommandSpawner` | `./src/launch.rs` |
| Launch↔window correlation, evidence tiers, expiry | `./src/correlate.rs` |
| `Exec` tokenization and field-code expansion | `./src/exec.rs` |
| `[Desktop Entry]` parsing, localized `Name`, `DesktopEntry` | `./src/parser.rs` |
| Desktop-file id derivation | `./src/app_id.rs` |
| `Terminal=true` wrapping | `./src/terminal.rs` |
| `TryExec` resolution | `./src/try_exec.rs` |
| XDG search-directory resolution | `./src/xdg.rs` |
| `Clock` seam and `MonotonicClock` | `./src/clock.rs` |
| Integration test suites + shared doubles/fixtures | `./tests/` (`support/mod.rs` + one file per concern) |

## Design Decisions

- Discovery: `search_dirs` order is precedence order — `$XDG_DATA_HOME/applications` first, then `$XDG_DATA_DIRS` entries (defaults `/usr/local/share`, `/usr/share`), each with `/applications` appended; relative/empty entries are ignored per the XDG spec, an unusable `HOME`/`XDG_DATA_HOME` falls back to the defaults, and identical directories are deduplicated first-wins.
- Explicit dirs replace defaults verbatim: `RegistryOptions::with_search_dirs(dirs)` overrides `search_dirs` with `dirs` exactly (the XDG defaults from `xdg()` are discarded by the struct-update syntax), and `scan` passes each given dir straight to `collect_desktop_files` — no `/applications` (or any other subdir) is appended. `/applications` joining exists only in `xdg::search_dirs_with` when computing defaults, and desktop-file ids are derived relative to the given dir.
- Deduplication: entries are keyed by desktop-file id, first search directory wins; a `Hidden=true` entry in a higher-precedence directory therefore shadows the same id in lower ones (the id resolves to the hidden entry, which `list` filters unless `include_hidden`).
- Scan is tolerant: unreadable/unparseable files, and entries missing `Name`/`Type`, become `ScanIssue`s; `Type != Application` files are counted in `skipped`; a scan never aborts on one bad file; missing search directories are silently skipped.
- `ScanReport`: `dirs_scanned` counts existing search dirs that were walked, `files_scanned` counts every `.desktop` file read (including shadowed and non-Application files), `apps` is the final table size.
- `scan(&self)` builds a fresh `BTreeMap` outside the lock and swaps it in, so readers never observe a half-built table; lock poisoning is recovered with `into_inner()` rather than panicking.
- Hidden-directory/file skipping: names starting with `.` are ignored, and directory symlinks are not followed (cycle safety); file symlinks are read normally.
- `list` filters `Hidden=true` and `NoDisplay=true` unless `include_hidden`; `get`/`launch` deliberately ignore those filters — filtering is a listing concern.
- `query` matches case-insensitively as a substring of the desktop-file id or the localized `Name`.
- App id: path relative to the applications dir, `.desktop` suffix removed, `/` → `.` (`kde/kate.desktop` → `kde.kate`); non-UTF-8 names, out-of-tree paths, `..` components and empty stems (`kde/.desktop`) yield `None`. `is_desktop_file` matches the byte suffix only, so a non-UTF-8 `*.desktop` name is a desktop file with no id.
- Parser: only `[Desktop Entry]` is read; unknown keys/groups and lines without `=` are ignored; duplicate keys keep the last occurrence; stage-1 escapes (`\s \n \t \r \\`) are resolved and unknown `\X` stays verbatim so `Exec` quoting (stage 2) still sees it; an empty localized value never shadows the base key.
- Localized `Name`: locale is normalized by dropping `.ENCODING`; candidate keys are tried in order `lang_COUNTRY@MODIFIER`, `lang@MODIFIER`, `lang_COUNTRY`, `lang`, then plain `Name`; only `Name` is localized (other keys use their base value).
- Booleans: only case-insensitive `true` is true; everything else is false. `Type` must be exactly `Application` (case-sensitive).
- `Exec` expansion is two stages with pinned tables (see `src/exec.rs` docs): quoting escapes in `tokenize` (single quotes are literal; only `\"`, `` \` ``, `\$`, `\\` escape, inside and outside quotes), field codes in `expand_tokens`; `%f/%u` take one file, `%F/%U` all files, `%i` is standalone-only (`--icon <icon>`), `%c`/`%k`/`%%` substitute in-token, deprecated `%d %D %n %N %v %m` are removed, unknown codes (including a lone `%`) and embedded file/icon codes stay verbatim, and empty tokens from quoting are preserved while tokens emptied by removal are dropped.
- `launch_app.args` are appended after expansion; they are not file arguments, so `%f/%F/%u/%U` expand to nothing in v1 (AGP carries no file list).
- `Terminal=true`: `$TERMINAL` is ASCII-whitespace-split, extra tokens go before the `-e` separator (`kitty --single-instance` → `kitty --single-instance -e`); unset/empty falls back to `x-terminal-emulator -e`; `TerminalSpec::new(program)` already includes `["-e"]` and `wrap` concatenates `prefix_args` then the command (`None` for an empty command), leaving `env` empty so `launch` applies `LaunchEnv`.
- `TryExec`: `PATH` is split on `:`, empty components are ignored (no implicit current-directory search), a value containing `/` is checked directly; unresolvable entries stay listed (their `try_exec` is visible in `AppInfo`; there is no availability field) and `launch` reports `TryExecNotFound` → AGP `launch_failed`.
- `DBusActivatable`: v1 always attempts the `Exec` line; `NoExec` → AGP `not_supported` only when the entry has no usable `Exec` (this covers DBus-only entries).
- Launch ids are monotonic from 1; they are allocated after validation, so `UnknownApp`/`TryExecNotFound`/`NoExec`/`InvalidExec` do not consume an id, while a failed spawn does (no reuse); `started_at_ms` comes from the injected `Clock` so tests control time.
- `CommandSpawner` runs the program directly (never through a shell), inherits the parent environment and applies `SpawnCommand::env` on top, returns the child pid, and does not wait/reap — the server owns reaping (see below). Command construction is testable via a private `build_command` helper without spawning.
- Correlation evidence tiers: exact pid → `StartupWMClass` == toplevel `app_id` (case-insensitive) → toplevel `app_id`/title contains the app id, its last dot-segment, or the entry `Name` (case-insensitive); within the winning tier the most recently started launch wins (ties: larger `launch_id`); expiry is strictly greater than the timeout, so a window mapping exactly at the deadline still correlates.
- A pending launch stays registered for its full timeout, so several windows of one launch can correlate; entries are pruned before every match, so a window never correlates with a stale launch; no match returns `Uncorrelated` (reported, never guessed).
- Registry and correlator MUST share one `Arc<dyn Clock>`: expiry compares `now_ms()` with `LaunchRecord::started_at_ms`.
- Error mapping to AGP (`Error::code`): `UnknownApp` → `unknown_app`, `NoExec` → `not_supported`, `InvalidExec`/`TryExecNotFound`/`Spawn` → `launch_failed`, `InvalidEntry`/`Io` → `internal`, `InvalidArgument` → `invalid_request`; `From<Error> for adesk_core::Error` uses the same mapping.

## What adesk-server must know

- Construct once at startup and share: `let clock: Arc<dyn Clock> = Arc::new(MonotonicClock::new()); let registry = Arc::new(AppRegistry::with_options(RegistryOptions::xdg().with_clock(clock.clone()))); let report = registry.scan()?;` and keep one `Correlator::new(clock)` on the event pump.
- `list_apps {query?, include_hidden?}` → `registry.list(query.as_deref(), include_hidden)`, respond `{"apps": [...]}`.
- `get_app {app_id}` → `registry.get(&AppId::from(id))`; `None` → `adesk_core::Error::unknown_app(&id)` (AGP `unknown_app`).
- `launch_app {app_id, args?}` → build `LaunchEnv::new()` with `with_wayland_display`/`with_xdg_runtime_dir` from the compositor, then `registry.launch(&app_id, &args, &env)`; respond `{"launch_id", "app_id", "pid"}` (`pid` may be null); errors go through `Error::code()` / `Into<adesk_core::Error>`.
- After a successful launch the server MUST (a) emit `AppLaunched { launch_id, app_id, pid }` on the event broadcast, and (b) call `correlator.record_launch(record.clone(), &app_info)` with the `AppInfo` from `registry.get`.
- On `WindowCreated`, build `WindowCandidate { window_id, pid, app_id, title }` and call `correlator.correlate`; on `Correlated` stamp the window's `app_id` (and announce it); on `Uncorrelated` leave `app_id: null` and log — never guess.
- `scan()` does blocking filesystem I/O and `launch()` spawns a process: call them from the server's tokio context (e.g. startup for `scan`, `spawn_blocking` if a rescan is ever exposed), never from the compositor thread.
- Child reaping is the server's responsibility: `CommandSpawner` returns the pid without waiting, so the server should reap (`tokio::process` child, or a `waitpid(WNOHANG)` task) to avoid zombie accumulation.
- `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` must be set on the launched process (build them into `LaunchEnv` from the compositor's socket), otherwise GUI clients cannot connect.

## Test Strategy

- Unit tests live inline (`#[cfg(test)] mod tests`) for pure logic with no integration counterpart: `xdg::search_dirs_with`, `terminal` env splitting, `try_exec` PATH search, `launch::LaunchEnv::overrides`, `CommandSpawner` command construction (`std::process::Command::get_envs`/`get_args`, no real spawn), and the registry's pure-logic pieces (option builders, `Send + Sync`).
- Integration tests live in `./tests/`, one file per concern, sharing `./tests/support/mod.rs` (recording mock `ProcessSpawner`, fake `Clock`, tempdir `.desktop` fixture writer, `AppInfo`/`LaunchRecord`/`WindowCandidate` builders):
  - `tests/parser.rs` (31) — happy path, localized `Name[fr]`/`Name[fr_FR]`, escapes, comments, duplicate keys, missing group, non-Application `Type`, boolean parsing.
  - `tests/exec.rs` (30) — quoting/escape table, `%f %F %u %U %i %c %k %%`, deprecated removal, unknown codes, empty quoted argument, unterminated quote.
  - `tests/app_id.rs` (13) — id derivation table, nested dirs, non-`.desktop`/out-of-tree paths.
  - `tests/registry.rs` (18) — tempdir scan/list/get, recursive subdirs, hidden/no_display filtering + `include_hidden`, precedence shadowing, query matching, launch argv/terminal/TryExec/env with the mock spawner, launch error paths (unknown app, no Exec, DBus without Exec, invalid Exec, spawn failure), monotonic launch ids, `started_at_ms` from the fake clock.
  - `tests/correlate.rs` (20) — pid → `StartupWMClass` → substring ordering, most-recent tie-break, timeout expiry, `Uncorrelated` reporting, multiple windows per launch.
- No test spawns a real process, needs a display, GPU, network, or an installed application; fixtures are written to `tempfile::tempdir()` (the single exception is `launch::command_spawner_reports_missing_program_as_io_error`, which attempts an OS spawn of a guaranteed-missing path and asserts the ENOENT `SpawnError::Io`).
- The integration files are the authoritative spec for `app_id`, `exec`, `parser` and `correlate`; their inline `#[cfg(test)]` modules were removed as strict-subset duplication, and the few inline-only assertions (correlate `pid`-evidence `app_id`, lower-tier fall-through, empty-needle rejection, the default-10s window and insertion-order pruning; parser raw pass-through plus key-generic/localized-only lookup) were folded into the integration files.
- The remaining 55 lib tests live inline in the modules with no integration counterpart (`clock` 4, `launch` 13, `registry` 2, `terminal` 14, `try_exec` 11, `xdg` 11) and are the sole coverage for those modules.
- No `#[ignore]`d tests exist anywhere in the crate, and no test mutates the process environment; the few ambient reads (`RegistryOptions::xdg` locale, `TerminalSpec::from_env`, `search_dirs`) are snapshot-and-compare, so the suite is parallel-safe with no shared mutable globals.
- Validation (always through the dev shell): `./scripts/dev.sh cargo test -p adesk-app-registry` (55 lib + 112 integration + 1 doctest = 168 passing, 0 ignored), `./scripts/dev.sh cargo clippy -p adesk-app-registry --all-targets --no-deps -- -D warnings` (clean), `./scripts/dev.sh cargo fmt --all --check` (clean workspace-wide), `./scripts/dev.sh cargo check --workspace --all-targets` (clean).

## Dependencies

- Internal: `adesk-core` (`AppId`, `AppInfo`, `LaunchId`, `WindowId`, `ErrorCode`, `Error`).
- External: `thiserror` (error enums), `tracing` (scan/launch diagnostics, never pixel data); dev-only `tempfile` (integration fixtures).
- System: none beyond ordinary filesystem/`PATH` access — no display, GPU, network, or Wayland socket needed for this crate's tests.

## Notes for Agents

- The desktop-file id rule is `/` → `.` (`kde/kate.desktop` → `kde.kate`), NOT `-`: `docs/architecture.md` §7 is normative and the `crates/adesk-testkit` fixtures assert the dotted form.
- `AppInfo` has no availability field, so `TryExec` failures surface at launch time (`TryExecNotFound`), not in `list_apps`; do not add fields to `adesk_core::AppInfo` — extend `docs/core-api.md` through the root instead.
- `AppRegistry::launch` never touches the `Correlator`: the server emits `AppLaunched` and records the launch itself (see "What adesk-server must know").
- `adesk-testkit` provides `.desktop` fixtures (`FixtureDir`, `TestApp`) and a helper process for end-to-end tests; this crate's own tests use `tempfile` directly and never spawn real processes.
- `RawEntry`, `DesktopEntry` and the `ProcessSpawner`/`Clock` traits are public so tests and `adesk-testkit` can build fixtures without reimplementing parsing or launch plumbing.
- `tests/support/mod.rs` carries a module-level `#![allow(dead_code)]` because it is compiled into five test binaries and each uses a subset of its helpers.
- `AppRegistry::launch` rejects an empty `argv` before building the command, so both `NoExec` arms inside the command construction (the `TerminalSpec::wrap` `None` case and the `argv.split_first()` `None` case) are unreachable defensive code, not real failure paths — do not chase coverage for them.
- `Error::Io`, `Error::InvalidEntry` and `Error::InvalidArgument` exist for API completeness but are never constructed by this crate: `scan()` reports per-file problems as `ScanIssue`s (it only returns `Ok`), and `launch()` can only fail with `UnknownApp`, `TryExecNotFound`, `NoExec`, `InvalidExec` or `Spawn`.

## Status

Every item documented above is implemented and exercised by the suites in Test Strategy:
- No `todo!()` in the crate, no crate-level `allow` attributes, and every file under the ~1000-line threshold (largest: `src/registry.rs`, 580 lines).
- `cargo test -p adesk-app-registry`: 55 lib + 112 integration + 1 doctest = 168 passing, 0 ignored.
- `cargo clippy -p adesk-app-registry --all-targets --no-deps -- -D warnings`, `cargo fmt --all --check`, `cargo check --workspace --all-targets` and `cargo doc -p adesk-app-registry --no-deps --document-private-items` are all clean (no warnings).
- Consumed by `adesk-server` (AGP dispatch, `AppLaunched` emission, correlation on the event pump) and `adesk-testkit` (`RunningServer::registry`).
