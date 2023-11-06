//! Provides a stub that retries requests based on response contents..

use crate::{
    client::{stub, RpcError},
    context,
};
use futures::prelude::*;
use std::sync::Arc;

impl<Stub, Req, F> stub::Stub for Retry<F, Stub>
where
    Stub: stub::Stub<Req = Arc<Req>>,
    F: Fn(&Result<Stub::Resp, RpcError>, u32) -> bool,
{
    type Req = Req;
    type Resp = Stub::Resp;
    type RespFut<'a> = RespFut<'a, Stub, Req, F>
        where Self: 'a,
              Self::Req: 'a;

    fn call<'a>(
        &'a self,
        ctx: context::Context,
        request_name: &'static str,
        request: Self::Req,
    ) -> Self::RespFut<'a> {
        Self::call(self, ctx, request_name, request)
    }
}

/// A type alias for a response future
pub type RespFut<'a, Stub: stub::Stub + 'a, Req: 'a, F: 'a> =
    impl Future<Output = Result<Stub::Resp, RpcError>> + 'a;

/// A Stub that retries requests based on response contents.
/// Note: to use this stub with Serde serialization, the "rc" feature of Serde needs to be enabled.
#[derive(Clone, Debug)]
pub struct Retry<F, Stub> {
    should_retry: F,
    stub: Stub,
}

impl<Stub, Req, F> Retry<F, Stub>
where
    Stub: stub::Stub<Req = Arc<Req>>,
    F: Fn(&Result<Stub::Resp, RpcError>, u32) -> bool,
{
    /// Creates a new Retry stub that delegates calls to the underlying `stub`.
    pub fn new(stub: Stub, should_retry: F) -> Self {
        Self { stub, should_retry }
    }

    async fn call<'a, 'b>(
        &'a self,
        ctx: context::Context,
        request_name: &'static str,
        request: Req,
    ) -> Result<Stub::Resp, RpcError>
    where
        Req: 'b,
    {
        let request = Arc::new(request);
        for i in 1.. {
            let result = self
                .stub
                .call(ctx.clone(), request_name, Arc::clone(&request))
                .await;
            if (self.should_retry)(&result, i) {
                tracing::trace!("Retrying on attempt {i}");
                continue;
            }
            return result;
        }
        unreachable!("Wow, that was a lot of attempts!");
    }
}
