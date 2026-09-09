//! §5.1 runtime methods.

use adesk_compositor::RendererName;
use adesk_proto::{PingParams, PingResult};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `ping`: liveness, protocol version and runtime identity.
///
/// Reports [`crate::PROTOCOL_VERSION`], [`crate::RUNTIME_VERSION`], uptime, the
/// renderer actually selected by the compositor and the virtual output size.
pub async fn ping(ctx: &RequestContext<'_>, params: PingParams) -> Result<PingResult> {
    let _ = params; // `ping` has no parameters (§5.1).

    // `renderer()` is `None` only before the compositor reports readiness, which
    // a bound socket normally rules out. The software path is the renderer that
    // always works headless (`docs/architecture.md` §5), so report it rather than
    // inventing a renderer value the protocol does not have.
    let renderer = ctx
        .server
        .compositor
        .renderer()
        .unwrap_or(RendererName::Pixman);

    Ok(PingResult {
        protocol_version: crate::PROTOCOL_VERSION,
        runtime_version: crate::RUNTIME_VERSION.to_owned(),
        uptime_ms: ctx.server.uptime_ms(),
        renderer: crate::translate::proto_renderer(renderer),
        output: ctx.server.compositor.output_size(),
    })
}
