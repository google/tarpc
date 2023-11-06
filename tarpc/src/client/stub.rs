//! Provides a Stub trait, implemented by types that can call remote services.

use crate::{
    client::{Channel, RpcError},
    context,
};
use futures::prelude::*;

pub mod load_balance;
pub mod retry;

#[cfg(test)]
mod mock;

/// A connection to a remote service.
/// Calls the service with requests of type `Req` and receives responses of type `Resp`.
pub trait Stub {
    /// The service request type.
    type Req;

    /// The service response type.
    type Resp;

    /// The type of the future returned by `Stub::call`.
    type RespFut<'a>: Future<Output = Result<Self::Resp, RpcError>>
    where
        Self: 'a,
        Self::Req: 'a,
        Self::Resp: 'a;

    /// Calls a remote service.
    fn call<'a>(
        &'a self,
        ctx: context::Context,
        request_name: &'static str,
        request: Self::Req,
    ) -> Self::RespFut<'a>;
}

impl<Req, Resp> Stub for Channel<Req, Resp> {
    type Req = Req;
    type Resp = Resp;
    type RespFut<'a> = RespFut<'a, Req, Resp>
        where Self: 'a;

    fn call<'a>(
        &'a self,
        ctx: context::Context,
        request_name: &'static str,
        request: Req,
    ) -> Self::RespFut<'a> {
        Self::call(self, ctx, request_name, request)
    }
}

/// A type alias for a response future
pub type RespFut<'a, Req: 'a, Resp: 'a> = impl Future<Output = Result<Resp, RpcError>> + 'a;
