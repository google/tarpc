use crate::{
    RequestName, ServerError,
    client::{RpcError, stub::Stub},
    context,
};
use std::{collections::HashMap, hash::Hash, io};

/// A mock stub that returns user-specified responses.
pub struct Mock<Req, Resp> {
    responses: HashMap<Req, Resp>,
}

impl<Req, Resp> Mock<Req, Resp>
where
    Req: Eq + Hash,
{
    /// Returns a new mock, mocking the specified (request, response) pairs.
    pub fn new<const N: usize>(responses: [(Req, Resp); N]) -> Self {
        Self {
            responses: HashMap::from(responses),
        }
    }
}

impl<Req, Resp> Stub for Mock<Req, Resp>
where
    Req: Eq + Hash + RequestName,
    Resp: Clone,
{
    type Req = Req;
    type Resp = Resp;

    async fn call(&self, _: &mut context::Context, request: Self::Req) -> Result<Resp, RpcError> {
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
