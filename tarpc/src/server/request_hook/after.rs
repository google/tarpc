// Copyright 2022 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a hook that runs after request execution.

use crate::{context, server::Serve, ServerError};
use futures::prelude::*;

/// A hook that runs after request execution.
pub trait AfterRequest<Resp> {
    /// The type of future returned by the hook.
    type Fut<'a>: Future<Output = ()>
    where
        Self: 'a,
        Resp: 'a;

    /// The function that is called after request execution.
    ///
    /// The hook can modify the request context and the response.
    fn after<'a>(
        &'a mut self,
        ctx: &'a mut context::Context,
        resp: &'a mut Result<Resp, ServerError>,
    ) -> Self::Fut<'a>;
}

impl<F, Fut, Resp> AfterRequest<Resp> for F
where
    F: FnMut(&mut context::Context, &mut Result<Resp, ServerError>) -> Fut,
    Fut: Future<Output = ()>,
{
    type Fut<'a> = Fut where Self: 'a, Resp: 'a;

    fn after<'a>(
        &'a mut self,
        ctx: &'a mut context::Context,
        resp: &'a mut Result<Resp, ServerError>,
    ) -> Self::Fut<'a> {
        self(ctx, resp)
    }
}

/// A Service function that runs a hook after request execution.
pub struct AfterRequestHook<Serv, Hook> {
    serve: Serv,
    hook: Hook,
}

impl<Serv, Hook> AfterRequestHook<Serv, Hook> {
    pub(crate) fn new(serve: Serv, hook: Hook) -> Self {
        Self { serve, hook }
    }
}

impl<Serv: Clone, Hook: Clone> Clone for AfterRequestHook<Serv, Hook> {
    fn clone(&self) -> Self {
        Self {
            serve: self.serve.clone(),
            hook: self.hook.clone(),
        }
    }
}

impl<Serv, Hook> Serve for AfterRequestHook<Serv, Hook>
where
    Serv: Serve,
    Hook: AfterRequest<Serv::Resp>,
{
    type Req = Serv::Req;
    type Resp = Serv::Resp;
    type Fut = AfterRequestHookFut<Serv, Hook>;

    fn serve(self, mut ctx: context::Context, req: Serv::Req) -> Self::Fut {
        async move {
            let AfterRequestHook {
                serve, mut hook, ..
            } = self;
            let mut resp = serve.serve(ctx, req).await;
            hook.after(&mut ctx, &mut resp).await;
            resp
        }
    }
}

type AfterRequestHookFut<Serv: Serve, Hook: AfterRequest<Serv::Resp>> =
    impl Future<Output = Result<Serv::Resp, ServerError>>;
