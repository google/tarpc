//! Provides a Stub trait, implemented by types that can call remote services.

use std::future::Future;

use crate::{
    client::{Channel, RpcError},
    context,
    server::{SendServe, Serve},
    RequestName,
};

pub mod load_balance;
pub mod retry;

#[cfg(test)]
mod mock;

/// A connection to a remote service.
/// Calls the service with requests of type `Req` and receives responses of type `Resp`.
pub trait Stub {
    /// The service request type.
    type Req: RequestName;

    /// The service response type.
    type Resp;

    /// Calls a remote service.
    fn call(
        &self,
        ctx: context::Context,
        request: Self::Req,
    ) -> impl Future<Output = Result<Self::Resp, RpcError>>;
}

/// A connection to a remote service.
/// Calls the service with requests of type `Req` and receives responses of type `Resp`.
pub trait SendStub: Send {
    /// The service request type.
    type Req: RequestName;

    /// The service response type.
    type Resp;

    /// Calls a remote service.
    fn call(
        &self,
        ctx: context::Context,
        request: Self::Req,
    ) -> impl Future<Output = Result<Self::Resp, RpcError>> + Send;
}

impl<Req, Resp> Stub for Channel<Req, Resp>
where
    Req: RequestName,
{
    type Req = Req;
    type Resp = Resp;

    async fn call(&self, ctx: context::Context, request: Req) -> Result<Self::Resp, RpcError> {
        Self::call(self, ctx, request).await
    }
}

impl<Req, Resp> SendStub for Channel<Req, Resp>
where
    Req: RequestName + Send,
    Resp: Send,
{
    type Req = Req;
    type Resp = Resp;

    async fn call(&self, ctx: context::Context, request: Req) -> Result<Self::Resp, RpcError> {
        Self::call(self, ctx, request).await
    }
}

impl<S> Stub for S
where
    S: Serve + Clone,
{
    type Req = S::Req;
    type Resp = S::Resp;
    async fn call(&self, ctx: context::Context, req: Self::Req) -> Result<Self::Resp, RpcError> {
        self.clone().serve(ctx, req).await.map_err(RpcError::Server)
    }
}

impl<S> SendStub for S
where
    S: SendServe + Clone + Sync,
    S::Req: Send + Sync,
    S::Resp: Send,
{
    type Req = S::Req;
    type Resp = S::Resp;
    async fn call(&self, ctx: context::Context, req: Self::Req) -> Result<Self::Resp, RpcError> {
        self.clone().serve(ctx, req).await.map_err(RpcError::Server)
    }
}
