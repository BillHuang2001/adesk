//! Server-level error type and its AGP mapping.
//!
//! Every dispatch arm returns [`Result`]; the dispatcher converts failures into
//! `ErrorPayload`s through [`ServerError::payload`] so a failed request never
//! closes the connection (`docs/protocol.md` §6).

use adesk_core::ErrorCode;

/// Failures that can abort or fail a server operation.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The compositor failed or is unavailable.
    #[error("compositor error: {0}")]
    Compositor(#[from] adesk_compositor::CompositorError),
    /// Socket or filesystem I/O failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The application registry failed.
    #[error("app registry error: {0}")]
    Registry(#[from] adesk_app_registry::Error),
    /// Frame encoding or decoding failed.
    #[error("protocol error: {0}")]
    Proto(#[from] adesk_proto::ProtoError),
    /// The observer rejected the request (unknown window/action, internal).
    #[error("observer error: {0}")]
    Observer(#[from] adesk_observer::Error),
    /// The inspector could not compose an inspection frame.
    #[error("inspector error: {0}")]
    Inspector(#[from] adesk_inspector::Error),
    /// Rendering or image encoding failed.
    #[error("render error: {0}")]
    Render(#[from] adesk_render::RenderError),
    /// The runtime is shutting down and cannot serve the request.
    #[error("server is shutting down")]
    ShuttingDown,
    /// A background task panicked or was cancelled.
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    /// An internal invariant was violated (never caused by client input).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience result for server operations.
///
/// The error type defaults to [`ServerError`], so `Result<T>` is the common
/// spelling while the startup/bind paths can name `ServerError` explicitly.
pub type Result<T, E = ServerError> = std::result::Result<T, E>;

impl ServerError {
    /// The AGP error code (`docs/protocol.md` §6) this failure maps to.
    ///
    /// The mapping is the server-side view of the sibling crates' pinned
    /// mappings: an observer request mistake is a client error, a renderer
    /// failure is a rendering failure, and anything the runtime cannot classify
    /// is `internal` — never a code invented here.
    ///
    /// Errors that a sibling crate already classifies are **delegated** rather
    /// than flattened: [`adesk_compositor::CompositorError::code`] and
    /// [`adesk_proto::ProtoError::error_code`] own the codes for the
    /// compositor- and transport-level failures, so a compositor-reported
    /// `unknown_window` or an unknown method is answered with its own AGP code
    /// instead of a blanket `internal`.
    pub fn code(&self) -> ErrorCode {
        match self {
            ServerError::Observer(error) => match error {
                adesk_observer::Error::UnknownWindow(_) => ErrorCode::UnknownWindow,
                adesk_observer::Error::UnknownAction(_) => ErrorCode::InvalidRequest,
                adesk_observer::Error::Internal(_) => ErrorCode::Internal,
            },
            ServerError::Registry(error) => match error {
                adesk_app_registry::Error::UnknownApp(_) => ErrorCode::UnknownApp,
                adesk_app_registry::Error::NoExec { .. } => ErrorCode::NotSupported,
                adesk_app_registry::Error::InvalidExec { .. }
                | adesk_app_registry::Error::TryExecNotFound { .. }
                | adesk_app_registry::Error::Spawn(_) => ErrorCode::LaunchFailed,
                adesk_app_registry::Error::InvalidEntry { .. }
                | adesk_app_registry::Error::Io { .. } => ErrorCode::Internal,
                // `adesk_app_registry::Error` is `#[non_exhaustive]`: anything
                // added later keeps the registry crate's own pinned mapping
                // instead of silently degrading to `internal`.
                _ => error.code(),
            },
            ServerError::Inspector(error) => match error {
                adesk_inspector::Error::InvalidFrame(_)
                | adesk_inspector::Error::InvalidRequest(_) => ErrorCode::InvalidRequest,
                adesk_inspector::Error::Render(_) => ErrorCode::RenderFailed,
            },
            // `adesk_render` owns the renderer-side mapping (`Encode` is a
            // `capture_failed`, caller mistakes are `invalid_request`).
            ServerError::Render(error) => error.code(),
            // `adesk_compositor` owns its own mapping: an unknown window and a
            // malformed request are client mistakes, `Render` is
            // `render_failed`, and the lifecycle variants (`NotReady`,
            // `StartupAborted`, `Stopped`) are `shutting_down` — exactly the
            // answer a request racing shutdown must get.
            ServerError::Compositor(error) => error.code(),
            // `adesk_proto` owns the wire mapping: an unknown method and a
            // version mismatch keep their codes, every other decode failure is
            // an `invalid_request`.
            ServerError::Proto(error) => error.error_code(),
            ServerError::ShuttingDown => ErrorCode::ShuttingDown,
            ServerError::Io(_) | ServerError::Join(_) | ServerError::Internal(_) => {
                ErrorCode::Internal
            }
        }
    }

    /// Builds the wire error payload for a failed request.
    ///
    /// `code`/`message` are always present; `data` carries the id that was not
    /// found for the three lookups where the client can act on it (§6 leaves
    /// `data` free-form, so no other variant invents a field). The ids are
    /// transparent newtypes over `u64`/`String`, so building the JSON cannot
    /// fail.
    pub fn payload(&self) -> adesk_proto::ErrorPayload {
        let payload = adesk_proto::ErrorPayload::new(self.code(), self.to_string());
        match self {
            ServerError::Observer(adesk_observer::Error::UnknownWindow(window_id)) => {
                payload.with_data(serde_json::json!({ "window_id": window_id }))
            }
            ServerError::Observer(adesk_observer::Error::UnknownAction(action_id)) => {
                payload.with_data(serde_json::json!({ "action_id": action_id }))
            }
            ServerError::Registry(adesk_app_registry::Error::UnknownApp(app_id)) => {
                payload.with_data(serde_json::json!({ "app_id": app_id }))
            }
            _ => payload,
        }
    }
}

impl From<ServerError> for adesk_core::Error {
    /// Lossy conversion used at crate boundaries: the AGP [`ErrorCode`] and the
    /// rendered message survive, while the structured `data` that
    /// [`ServerError::payload`] attaches (window/action/app ids) is dropped —
    /// `adesk_core::Error` has no `data` field. Use [`ServerError::payload`]
    /// whenever the wire error object is what matters.
    fn from(error: ServerError) -> adesk_core::Error {
        adesk_core::Error::new(error.code(), error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use adesk_core::{ActionId, AppId, WindowId};

    use super::*;

    /// One representative instance of every [`ServerError`] variant with the
    /// code `docs/protocol.md` §6 requires.
    ///
    /// `Join` is absent because a [`tokio::task::JoinError`] cannot be built
    /// directly; `join_error_maps_to_internal` produces a real one.
    fn cases() -> Vec<(ServerError, ErrorCode)> {
        vec![
            (
                ServerError::Observer(adesk_observer::Error::UnknownWindow(WindowId(7))),
                ErrorCode::UnknownWindow,
            ),
            (
                ServerError::Observer(adesk_observer::Error::UnknownAction(ActionId(582))),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Observer(adesk_observer::Error::Internal("state dropped".into())),
                ErrorCode::Internal,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::UnknownApp(AppId::from(
                    "org.mozilla.firefox",
                ))),
                ErrorCode::UnknownApp,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::NoExec {
                    app: AppId::from("org.example.dbus"),
                }),
                ErrorCode::NotSupported,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::InvalidExec {
                    app: AppId::from("org.example.bad"),
                    source: adesk_app_registry::ExecError::UnterminatedQuote { offset: 3 },
                }),
                ErrorCode::LaunchFailed,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::TryExecNotFound {
                    program: "missing-binary".into(),
                }),
                ErrorCode::LaunchFailed,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::Spawn(
                    adesk_app_registry::SpawnError::Other("mock spawner failed".into()),
                )),
                ErrorCode::LaunchFailed,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::InvalidEntry {
                    path: PathBuf::from("/usr/share/applications/bad.desktop"),
                    reason: "missing Name".into(),
                }),
                ErrorCode::Internal,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::Io {
                    path: PathBuf::from("/usr/share/applications"),
                    source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
                }),
                ErrorCode::Internal,
            ),
            (
                ServerError::Registry(adesk_app_registry::Error::InvalidArgument(
                    "limit must be positive".into(),
                )),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Inspector(adesk_inspector::Error::InvalidFrame(
                    "size mismatch".into(),
                )),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Inspector(adesk_inspector::Error::InvalidRequest(
                    "empty region".into(),
                )),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Inspector(adesk_inspector::Error::Render(
                    adesk_render::RenderError::Encode {
                        reason: "png failed".into(),
                    },
                )),
                ErrorCode::RenderFailed,
            ),
            (
                ServerError::Render(adesk_render::RenderError::Encode {
                    reason: "png failed".into(),
                }),
                ErrorCode::CaptureFailed,
            ),
            (
                ServerError::Render(adesk_render::RenderError::InvalidConfig {
                    reason: "crop outside source".into(),
                }),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Render(adesk_render::RenderError::InvalidImage {
                    reason: "short buffer".into(),
                }),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Render(adesk_render::RenderError::ImportFailed {
                    reason: "dmabuf not sampleable".into(),
                }),
                ErrorCode::RenderFailed,
            ),
            (
                ServerError::Compositor(adesk_compositor::CompositorError::UnknownWindow(
                    WindowId(7),
                )),
                ErrorCode::UnknownWindow,
            ),
            (
                ServerError::Compositor(adesk_compositor::CompositorError::InvalidRequest(
                    "no focused window".into(),
                )),
                ErrorCode::InvalidRequest,
            ),
            (
                ServerError::Compositor(adesk_compositor::CompositorError::Render(
                    "readback failed".into(),
                )),
                ErrorCode::RenderFailed,
            ),
            (
                ServerError::Compositor(adesk_compositor::CompositorError::Stopped),
                ErrorCode::ShuttingDown,
            ),
            (
                ServerError::Compositor(adesk_compositor::CompositorError::Internal("bug".into())),
                ErrorCode::Internal,
            ),
            (
                ServerError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")),
                ErrorCode::Internal,
            ),
            (
                ServerError::Proto(adesk_proto::ProtoError::UnknownMethod(
                    "definitely_not_a_method".into(),
                )),
                ErrorCode::UnknownMethod,
            ),
            (
                ServerError::Proto(adesk_proto::ProtoError::VersionMismatch {
                    expected: 1,
                    got: 2,
                }),
                ErrorCode::ProtocolVersionMismatch,
            ),
            (
                ServerError::Proto(adesk_proto::ProtoError::Malformed("garbage".into())),
                ErrorCode::InvalidRequest,
            ),
            (ServerError::ShuttingDown, ErrorCode::ShuttingDown),
            (
                ServerError::Internal("invariant violated".into()),
                ErrorCode::Internal,
            ),
        ]
    }

    #[test]
    fn code_matches_protocol_table() {
        for (error, expected) in cases() {
            assert_eq!(error.code(), expected, "wrong code for {error:?}");
        }
    }

    /// One instance of every [`adesk_compositor::CompositorError`] variant.
    ///
    /// The compositor owns this mapping ([`adesk_compositor::CompositorError::code`]);
    /// the server must delegate to it instead of flattening the error into
    /// `internal`, so the whole table is pinned here.
    fn compositor_cases() -> Vec<(adesk_compositor::CompositorError, ErrorCode)> {
        vec![
            (
                adesk_compositor::CompositorError::ThreadSpawn(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "spawn failed",
                )),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::Display("no display".into()),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::Socket("bind failed".into()),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::EventLoop("loop failed".into()),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::Renderer("no renderer".into()),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::Keyboard("no keymap".into()),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::NotReady,
                ErrorCode::ShuttingDown,
            ),
            (
                adesk_compositor::CompositorError::StartupAborted,
                ErrorCode::ShuttingDown,
            ),
            (
                adesk_compositor::CompositorError::Stopped,
                ErrorCode::ShuttingDown,
            ),
            (
                adesk_compositor::CompositorError::UnknownWindow(WindowId(7)),
                ErrorCode::UnknownWindow,
            ),
            (
                adesk_compositor::CompositorError::InvalidRequest("no focused window".into()),
                ErrorCode::InvalidRequest,
            ),
            (
                adesk_compositor::CompositorError::WindowManagement("tiling failed".into()),
                ErrorCode::Internal,
            ),
            (
                adesk_compositor::CompositorError::Render("readback failed".into()),
                ErrorCode::RenderFailed,
            ),
            (
                adesk_compositor::CompositorError::Internal("bug".into()),
                ErrorCode::Internal,
            ),
        ]
    }

    #[test]
    fn compositor_errors_keep_the_compositor_mapping() {
        for (error, expected) in compositor_cases() {
            let server = ServerError::Compositor(error);
            assert_eq!(server.code(), expected, "wrong code for {server:?}");
            assert_eq!(
                server.payload().code,
                expected,
                "wrong payload code for {server:?}"
            );
        }
    }

    /// One instance of every [`adesk_proto::ProtoError`] variant.
    ///
    /// `adesk-proto` owns this mapping ([`adesk_proto::ProtoError::error_code`]);
    /// the transport layer must not answer `internal` for a decode failure a
    /// client can act on.
    fn proto_cases() -> Vec<(adesk_proto::ProtoError, ErrorCode)> {
        let json = serde_json::from_str::<serde_json::Value>("{not json")
            .expect_err("truncated JSON must fail to parse");
        let base64 = adesk_proto::ImagePayload {
            width: 1,
            height: 1,
            format: adesk_proto::ImageFormat::Png,
            stride: None,
            data: "not base64!".to_owned(),
            scale: 1.0,
        }
        .decode_data()
        .expect_err("invalid base64 must fail to decode");
        let base64 = match base64 {
            adesk_proto::ProtoError::Base64(error) => error,
            other => panic!("invalid base64 must fail with ProtoError::Base64, got {other:?}"),
        };
        vec![
            (
                adesk_proto::ProtoError::Malformed("garbage".into()),
                ErrorCode::InvalidRequest,
            ),
            (
                adesk_proto::ProtoError::UnknownMethod("definitely_not_a_method".into()),
                ErrorCode::UnknownMethod,
            ),
            (
                adesk_proto::ProtoError::InvalidParams {
                    method: "click".into(),
                    message: "missing window_id".into(),
                },
                ErrorCode::InvalidRequest,
            ),
            (
                adesk_proto::ProtoError::InvalidEventData {
                    kind: "window_created".into(),
                    message: "missing window".into(),
                },
                ErrorCode::InvalidRequest,
            ),
            (
                adesk_proto::ProtoError::UnknownEventKind("inspect_frame".into()),
                ErrorCode::InvalidRequest,
            ),
            (
                adesk_proto::ProtoError::InvalidResult("not an object".into()),
                ErrorCode::InvalidRequest,
            ),
            (
                adesk_proto::ProtoError::VersionMismatch {
                    expected: 1,
                    got: 2,
                },
                ErrorCode::ProtocolVersionMismatch,
            ),
            (adesk_proto::ProtoError::Json(json), ErrorCode::InvalidRequest),
            (
                adesk_proto::ProtoError::Base64(base64),
                ErrorCode::InvalidRequest,
            ),
        ]
    }

    #[test]
    fn proto_errors_keep_the_protocol_mapping() {
        for (error, expected) in proto_cases() {
            let server = ServerError::Proto(error);
            assert_eq!(server.code(), expected, "wrong code for {server:?}");
            assert_eq!(
                server.payload().code,
                expected,
                "wrong payload code for {server:?}"
            );
        }
    }

    #[test]
    fn payload_carries_code_and_display_message() {
        for (error, expected) in cases() {
            let payload = error.payload();
            assert_eq!(payload.code, expected);
            assert_eq!(payload.message, error.to_string());
        }
    }

    #[test]
    fn payload_data_carries_unknown_window_id() {
        let error = ServerError::Observer(adesk_observer::Error::UnknownWindow(WindowId(7)));
        let payload = error.payload();
        assert_eq!(payload.code, ErrorCode::UnknownWindow);
        assert_eq!(payload.data, Some(serde_json::json!({ "window_id": 7 })));
    }

    #[test]
    fn payload_data_carries_unknown_action_id() {
        let error = ServerError::Observer(adesk_observer::Error::UnknownAction(ActionId(582)));
        let payload = error.payload();
        assert_eq!(payload.code, ErrorCode::InvalidRequest);
        assert_eq!(payload.data, Some(serde_json::json!({ "action_id": 582 })));
    }

    #[test]
    fn payload_data_carries_unknown_app_id_as_bare_string() {
        let error = ServerError::Registry(adesk_app_registry::Error::UnknownApp(AppId::from(
            "org.mozilla.firefox",
        )));
        let payload = error.payload();
        assert_eq!(payload.code, ErrorCode::UnknownApp);
        // `AppId` is `#[serde(transparent)]`, so the id is the bare string.
        assert_eq!(
            payload.data,
            Some(serde_json::json!({ "app_id": "org.mozilla.firefox" }))
        );
    }

    #[test]
    fn payload_omits_data_for_every_other_variant() {
        let variants = vec![
            ServerError::Observer(adesk_observer::Error::Internal("boom".into())),
            ServerError::Registry(adesk_app_registry::Error::NoExec {
                app: AppId::from("org.example.dbus"),
            }),
            ServerError::Registry(adesk_app_registry::Error::InvalidEntry {
                path: PathBuf::from("/bad.desktop"),
                reason: "missing Name".into(),
            }),
            ServerError::Inspector(adesk_inspector::Error::InvalidRequest("empty".into())),
            ServerError::Render(adesk_render::RenderError::Encode {
                reason: "png".into(),
            }),
            ServerError::Compositor(adesk_compositor::CompositorError::Stopped),
            ServerError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone")),
            ServerError::Proto(adesk_proto::ProtoError::Malformed("garbage".into())),
            ServerError::ShuttingDown,
            ServerError::Internal("invariant violated".into()),
        ];
        for error in variants {
            assert_eq!(error.payload().data, None, "unexpected data for {error:?}");
        }
    }

    #[test]
    fn payload_serializes_as_the_agp_error_object() {
        let plain = ServerError::ShuttingDown.payload();
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::json!({
                "code": "shutting_down",
                "message": "server is shutting down",
            })
        );

        let with_data =
            ServerError::Observer(adesk_observer::Error::UnknownWindow(WindowId(99))).payload();
        assert_eq!(
            serde_json::to_value(&with_data).unwrap(),
            serde_json::json!({
                "code": "unknown_window",
                "message": "observer error: window 99 is not known",
                "data": { "window_id": 99 },
            })
        );
    }

    #[test]
    fn into_core_error_preserves_code_and_message() {
        for (error, expected) in cases() {
            let message = error.to_string();
            let core: adesk_core::Error = error.into();
            assert_eq!(core.code, expected);
            assert_eq!(core.message, message);
        }
    }

    #[tokio::test]
    async fn join_error_maps_to_internal() {
        let handle = tokio::spawn(std::future::pending::<()>());
        handle.abort();
        let join_error = handle.await.expect_err("aborted task must fail to join");

        let error = ServerError::Join(join_error);
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.payload().code, ErrorCode::Internal);
        assert_eq!(error.payload().data, None);

        let core: adesk_core::Error = error.into();
        assert_eq!(core.code, ErrorCode::Internal);
    }
}
