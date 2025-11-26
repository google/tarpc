//! Provides a Stub trait, implemented by types that can call remote services.

use crate::context::{ExtractContext, SharedContext};
use crate::{
    RequestName,
    client::{Channel, RpcError},
    server::Serve,
};

pub mod load_balance;
pub mod retry;

#[cfg(test)]
mod mock;

/// A connection to a remote service.
/// Calls the service with requests of type `Req` and receives responses of type `Resp`.
#[allow(async_fn_in_trait)]
pub trait Stub {
    /// The service request type.
    type Req: RequestName;

    /// The service response type.
    type Resp;

    ///TODO: document
    type ClientCtx;

    /// Calls a remote service.
    async fn call(
        &self,
        ctx: &mut Self::ClientCtx,
        request: Self::Req,
    ) -> Result<Self::Resp, RpcError>;
}

impl<Req, Resp, ClientCtx> Stub for Channel<Req, Resp, ClientCtx>
where
    Req: RequestName,
    ClientCtx: ExtractContext<SharedContext>,
{
    type Req = Req;
    type Resp = Resp;
    type ClientCtx = ClientCtx;

    async fn call(&self, ctx: &mut Self::ClientCtx, request: Req) -> Result<Self::Resp, RpcError> {
        Self::call(self, ctx, request).await
    }
}

impl<S> Stub for S
where
    S: Serve<ServerCtx = SharedContext> + Clone,
{
    type Req = S::Req;
    type Resp = S::Resp;
    type ClientCtx = SharedContext;
    async fn call(
        &self,
        ctx: &mut Self::ClientCtx,
        req: Self::Req,
    ) -> Result<Self::Resp, RpcError> {
        let mut server_ctx = ctx.clone();

        let res = self
            .clone()
            .serve(&mut server_ctx, req)
            .await
            .map_err(RpcError::Server);

        *ctx = server_ctx;

        res
    }
}
