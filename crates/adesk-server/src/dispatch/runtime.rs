//! §5.1 runtime methods.

use adesk_proto::{PingParams, PingResult};

use crate::dispatch::RequestContext;
use crate::error::Result;

/// `ping`: liveness, protocol version and runtime identity.
///
/// Reports [`crate::PROTOCOL_VERSION`], [`crate::RUNTIME_VERSION`], uptime, the
/// renderer actually selected by the compositor and the virtual output size.
pub async fn ping(ctx: &RequestContext<'_>, params: PingParams) -> Result<PingResult> {
    todo!()
}
