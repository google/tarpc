// Copyright 2022 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a hook that runs before request execution.

use crate::{context, server::Serve, ServerError};
use futures::prelude::*;

/// A hook that runs before request execution.
#[allow(async_fn_in_trait)]
pub trait BeforeRequest<Req> {
    /// The function that is called before request execution.
    ///
    /// If this function returns an error, the request will not be executed and the error will be
    /// returned instead.
    ///
    /// This function can also modify the request context. This could be used, for example, to
    /// enforce a maximum deadline on all requests.
    async fn before(&mut self, ctx: &mut context::Context, req: &Req) -> Result<(), ServerError>;
}

impl<F, Fut, Req> BeforeRequest<Req> for F
where
    F: FnMut(&mut context::Context, &Req) -> Fut,
    Fut: Future<Output = Result<(), ServerError>>,
{
    async fn before(&mut self, ctx: &mut context::Context, req: &Req) -> Result<(), ServerError> {
        self(ctx, req).await
    }
}

/// A Service function that runs a hook before request execution.
pub struct BeforeRequestHook<Serv, Hook> {
    serve: Serv,
    hook: Hook,
}

impl<Serv, Hook> BeforeRequestHook<Serv, Hook> {
    pub(crate) fn new(serve: Serv, hook: Hook) -> Self {
        Self { serve, hook }
    }
}

impl<Serv: Clone, Hook: Clone> Clone for BeforeRequestHook<Serv, Hook> {
    fn clone(&self) -> Self {
        Self {
            serve: self.serve.clone(),
            hook: self.hook.clone(),
        }
    }
}

impl<Serv, Hook> Serve for BeforeRequestHook<Serv, Hook>
where
    Serv: Serve,
    Hook: BeforeRequest<Serv::Req>,
{
    type Req = Serv::Req;
    type Resp = Serv::Resp;

    async fn serve(
        self,
        mut ctx: context::Context,
        req: Self::Req,
    ) -> Result<Serv::Resp, ServerError> {
        let BeforeRequestHook {
            serve, mut hook, ..
        } = self;
        hook.before(&mut ctx, &req).await?;
        serve.serve(ctx, req).await
    }
}
