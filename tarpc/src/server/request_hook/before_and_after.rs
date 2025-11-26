// Copyright 2022 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a hook that runs both before and after request execution.

use super::{after::AfterRequest, before::BeforeRequest};
use crate::{RequestName, ServerError, server::Serve};
use std::marker::PhantomData;

/// A Service function that runs a hook both before and after request execution.
pub struct HookThenServeThenHook<Req, Resp, Serv, Hook, ServerCtx> {
    serve: Serv,
    hook: Hook,
    fns: PhantomData<(Req, Resp, ServerCtx)>,
}

impl<Req, Resp, Serv, Hook, ServerCtx> HookThenServeThenHook<Req, Resp, Serv, Hook, ServerCtx> {
    pub(crate) fn new(serve: Serv, hook: Hook) -> Self {
        Self {
            serve,
            hook,
            fns: PhantomData,
        }
    }
}

impl<Req, Resp, Serv: Clone, Hook: Clone, ServerCtx> Clone for HookThenServeThenHook<Req, Resp, Serv, Hook, ServerCtx> {
    fn clone(&self) -> Self {
        Self {
            serve: self.serve.clone(),
            hook: self.hook.clone(),
            fns: PhantomData,
        }
    }
}

impl<Req, Resp, Serv, Hook, ServerCtx> Serve for HookThenServeThenHook<Req, Resp, Serv, Hook, ServerCtx>
where
    Req: RequestName,
    Serv: Serve<ServerCtx = ServerCtx, Req = Req, Resp = Resp>,
    Hook: BeforeRequest<ServerCtx, Req> + AfterRequest<ServerCtx, Resp>,
{
    type Req = Req;
    type Resp = Resp;
    type ServerCtx = ServerCtx;

    async fn serve(
        self,
        ctx: &mut ServerCtx,
        req: Req,
    ) -> Result<Serv::Resp, ServerError> {
        let HookThenServeThenHook {
            serve, mut hook, ..
        } = self;
        hook.before(ctx, &req).await?;
        let mut resp = serve.serve(ctx, req).await;
        hook.after(ctx, &mut resp).await;
        resp
    }
}
