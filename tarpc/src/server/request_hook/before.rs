// Copyright 2022 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a hook that runs before request execution.

use crate::{context, server::Serve, ServerError};
use futures::prelude::*;

/// A hook that runs before request execution.
pub trait BeforeRequest<Req> {
    /// The type of future returned by the hook.
    type Fut<'a>: Future<Output = Result<(), ServerError>>
    where
        Self: 'a,
        Req: 'a;

    /// The function that is called before request execution.
    ///
    /// If this function returns an error, the request will not be executed and the error will be
    /// returned instead.
    ///
    /// This function can also modify the request context. This could be used, for example, to
    /// enforce a maximum deadline on all requests.
    fn before<'a>(&'a mut self, ctx: &'a mut context::Context, req: &'a Req) -> Self::Fut<'a>;
}

impl<F, Fut, Req> BeforeRequest<Req> for F
where
    F: FnMut(&mut context::Context, &Req) -> Fut,
    Fut: Future<Output = Result<(), ServerError>>,
{
    type Fut<'a> = Fut where Self: 'a, Req: 'a;

    fn before<'a>(&'a mut self, ctx: &'a mut context::Context, req: &'a Req) -> Self::Fut<'a> {
        self(ctx, req)
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
        ctx: &mut context::Context,
        req: Self::Req,
    ) -> Result<Serv::Resp, ServerError> {
        let BeforeRequestHook {
            serve, mut hook, ..
        } = self;
        hook.before(ctx, &req).await?;
        serve.serve(ctx, req).await
    }
}
