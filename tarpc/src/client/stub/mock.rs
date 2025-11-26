use crate::{
    RequestName, ServerError,
    client::{RpcError, stub::Stub},
};
use std::{collections::HashMap, hash::Hash, io};
use std::marker::PhantomData;

/// A mock stub that returns user-specified responses.
pub struct Mock<Req, Resp, ServerCtx> {
    responses: HashMap<Req, Resp>,
    ghost: PhantomData<ServerCtx>
}

impl<Req, Resp, ServerCtx> Mock<Req, Resp, ServerCtx>
where
    Req: Eq + Hash,
{
    /// Returns a new mock, mocking the specified (request, response) pairs.
    pub fn new<const N: usize>(responses: [(Req, Resp); N]) -> Self {
        Self {
            responses: HashMap::from(responses),
            ghost: PhantomData
        }
    }
}

impl<Req, Resp, ServerCtx> Stub for Mock<Req, Resp, ServerCtx>
where
    Req: Eq + Hash + RequestName,
    Resp: Clone,
{
    type Req = Req;
    type Resp = Resp;
    type ClientCtx = ServerCtx;

    async fn call(
        &self,
        _: &mut Self::ClientCtx,
        request: Self::Req,
    ) -> Result<Resp, RpcError> {
        self.responses
            .get(&request)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                Err(RpcError::Server(ServerError {
                    kind: io::ErrorKind::NotFound,
                    detail: "mock (request, response) entry not found".into(),
                }))
            })
    }
}
