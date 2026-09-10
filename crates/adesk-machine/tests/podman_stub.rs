//! [`PodmanRuntime`] integration tests against a temporary `#!/bin/sh` stub.
//!
//! The stub stands in for the `podman` CLI, so the backend's argv translation,
//! stdout parsing and error mapping are exercised end to end without a real
//! container engine. Unix-only, and only `/bin/sh` is required.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use adesk_machine::runtime::podman::build_create_argv;
use adesk_machine::{
    ContainerRuntime, MachineError, MachineId, MachineName, MachineSpec, MachineState,
    PodmanRuntime,
};

/// Serializes the tests that spawn a child process.
///
/// A `fork` in one test can inherit another test's still-open write descriptor
/// to a stub script, which makes a concurrent `exec` of that script fail with
/// `ETXTBSY` ("Text file busy"). Holding this lock across both stub creation and
/// spawning removes the race.
static SPAWN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Writes `script` as an executable `stub-podman` inside `dir` and returns its
/// path.
///
/// The write handle is flushed and dropped before returning, so a later `exec`
/// of the script never races the writer.
fn write_stub_in(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("stub-podman");
    {
        let mut file = std::fs::File::create(&path).expect("create stub");
        file.write_all(script.as_bytes()).expect("write stub");
        file.sync_all().expect("flush stub");
    }
    let mut permissions = std::fs::metadata(&path).expect("stat stub").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod stub");
    path
}

/// Writes a stub in its own [`tempfile::TempDir`], returning the dir (kept alive
/// by the caller) and the stub's path.
fn write_stub(script: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_stub_in(dir.path(), script);
    (dir, path)
}

#[tokio::test]
async fn create_start_stop_and_remove_drive_the_stub() {
    let _guard = SPAWN_LOCK.lock().await;
    let (_dir, program) = write_stub(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         create) printf 'deadbeefcafe\\n' ;;\n\
         start|stop|rm) exit 0 ;;\n\
         *) exit 2 ;;\n\
         esac\n",
    );
    let runtime = PodmanRuntime::with_program(program);
    let spec = MachineSpec::default();

    // The container id is read from `create`'s stdout.
    let id = runtime.create(&spec).await.unwrap();
    assert_eq!(id, MachineId::from("deadbeefcafe"));

    // The mutating subcommands succeed against the stub.
    runtime.start(&id).await.unwrap();
    runtime.stop(&id, 1_500).await.unwrap();
    runtime.remove(&id, true).await.unwrap();
}

#[tokio::test]
async fn status_parses_the_inspect_payload() {
    let _guard = SPAWN_LOCK.lock().await;
    let (_dir, program) = write_stub(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         inspect) printf '%s\\n' '[{\"Id\":\"deadbeefcafe\",\"Name\":\"adesk\",\
         \"Image\":\"ghcr.io/adesk/machine:latest\",\
         \"Created\":\"2024-06-01T12:00:00.500000000Z\",\
         \"State\":{\"Status\":\"running\",\"Pid\":4242,\"ExitCode\":0}}]' ;;\n\
         *) exit 2 ;;\n\
         esac\n",
    );
    let runtime = PodmanRuntime::with_program(program);

    let status = runtime
        .status(&MachineId::from("deadbeefcafe"))
        .await
        .unwrap();
    assert_eq!(status.id, MachineId::from("deadbeefcafe"));
    assert_eq!(status.name, MachineName::from("adesk"));
    assert_eq!(status.image, "ghcr.io/adesk/machine:latest");
    assert_eq!(status.state, MachineState::Running);
    assert_eq!(status.pid, Some(4_242));
    assert_eq!(status.created_at_ms, 1_717_243_200_500);
}

#[tokio::test]
async fn create_passes_the_built_argv_to_the_backend() {
    let _guard = SPAWN_LOCK.lock().await;

    let dir = tempfile::tempdir().expect("temp dir");
    // The stub records the argv it received here; the test pins the translation.
    let record = dir.path().join("create-argv.txt");
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         create)\n\
         printf '%s\\n' \"$@\" > {}\n\
         printf 'deadbeefcafe\\n'\n\
         ;;\n\
         *) exit 2 ;;\n\
         esac\n",
        record.display()
    );
    let program = write_stub_in(dir.path(), &script);

    let runtime = PodmanRuntime::with_program(program);
    let spec = MachineSpec::default();
    let id = runtime.create(&spec).await.unwrap();
    assert_eq!(id, MachineId::from("deadbeefcafe"));

    let recorded = std::fs::read_to_string(&record).expect("recorded argv");
    let args: Vec<String> = recorded.lines().map(str::to_owned).collect();
    assert_eq!(args, build_create_argv(&spec), "argv sent to the stub");
}

#[tokio::test]
async fn non_zero_exit_maps_to_a_backend_error() {
    let _guard = SPAWN_LOCK.lock().await;
    let (_dir, program) = write_stub("#!/bin/sh\necho 'refusing to stop' 1>&2\nexit 3\n");
    let runtime = PodmanRuntime::with_program(program);

    let err = runtime
        .stop(&MachineId::from("machine-1"), 0)
        .await
        .unwrap_err();
    match err {
        MachineError::Backend {
            program,
            status,
            stderr,
        } => {
            assert_eq!(status, 3);
            assert_eq!(stderr, "refusing to stop");
            assert!(program.ends_with("stub-podman"), "program was {program}");
        }
        other => panic!("expected a backend error, got {other:?}"),
    }
}
