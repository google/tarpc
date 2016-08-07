use {bincode, futures};
use std::{error, fmt, io};
use tokio::proto::pipeline;

quick_error! {
    /// All errors that can occur during the use of tarpc.
    #[derive(Debug)]
    pub enum Error {
        /// No address found for the specified address.
        /// Depending on the outcome of address resolution, `ToSocketAddrs` may not yield any
        /// values, which will propagate as this variant.
        NoAddressFound {}
        /// The client connected to a tarpc service that did not recognize the client request.
        WrongService(desc: String) {
            description(&desc)
        }
        /// Any IO error other than ConnectionBroken.
        Io(err: io::Error) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in deserializing, either on client or server.
        Deserialize(err: bincode::serde::DeserializeError) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in serializing, either on client or server.
        Serialize(err: bincode::serde::SerializeError) {
            cause(err)
            from()
            description(err.description())
        }
        /// The server was unable to reply to the rpc for some reason.
        Rpc(err: RpcError) {
            cause(err)
            from()
            description(err.description())
        }
    }
}

impl From<CanonicalRpcError> for Error {
    fn from(err: CanonicalRpcError) -> Self {
        info!("Err: {:?}", err);
        match err.code {
            CanonicalRpcErrorCode::Service(code) => {
                Error::Rpc(RpcError {
                    code: code,
                    description: err.description,
                })
            }
            CanonicalRpcErrorCode::WrongService => Error::WrongService(err.description),
        }
    }
}

/// A serializable, server-supplied error.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RpcError {
    /// The type of error that occurred.
    pub code: RpcErrorCode,
    /// More details about the error.
    pub description: String,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.description)
    }
}

impl error::Error for RpcError {
    fn description(&self) -> &str {
        &self.description
    }
}

/// A serializable, server-supplied error that is a superset of `RpcError`. In addition to
/// transmitting errors that the service writer may concern themselves with, it also transmits
/// some errors automatically. For example, if request deserialization fails, the framework
/// will automatically respond with a `WrongService` error, which all tarpc clients will
/// recognize.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CanonicalRpcError {
    /// The type of error that occurred.
    pub code: CanonicalRpcErrorCode,
    /// More details about the error.
    pub description: String,
}

impl fmt::Display for CanonicalRpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.description)
    }
}

impl error::Error for CanonicalRpcError {
    fn description(&self) -> &str {
        &self.description
    }
}

/// Combines error codes propagated automatically by the tarpc framework, as well as error codes
/// propagated by the service. Is a superset of `RpcErrorCode`.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum CanonicalRpcErrorCode {
    /// The service returned an error.
    Service(RpcErrorCode),
    /// The service could not interpret the request.
    WrongService,
}

impl fmt::Display for CanonicalRpcErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CanonicalRpcErrorCode::Service(ref code) => write!(f, "{}", code),
            CanonicalRpcErrorCode::WrongService => write!(f, "Wrong service"),
        }
    }
}

impl From<futures::Canceled> for RpcError {
    fn from(_: futures::Canceled) -> Self {
        RpcError {
            code: RpcErrorCode::Internal,
            description: "The server failed to respond.".to_string(),
        }
    }
}

impl From<Error> for CanonicalRpcError {
    fn from(err: Error) -> Self {
        match err {
            Error::WrongService(desc) => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::WrongService,
                    description: desc,
                }
            }
            Error::Rpc(e) => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::Service(e.code),
                    description: e.description,
                }
            }
            e => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::Service(RpcErrorCode::Internal),
                    description: error::Error::description(&e).to_string(),
                }
            }
        }
    }
}

impl From<RpcError> for CanonicalRpcError {
    fn from(err: RpcError) -> Self {
        CanonicalRpcError {
            description: err.description,
            code: CanonicalRpcErrorCode::Service(err.code),
        }
    }
}

/// Reasons an rpc failed.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcErrorCode {
    /// An internal error occurred on the server.
    Internal,
    /// The user input failed a precondition of the rpc method.
    BadRequest,
}

impl fmt::Display for RpcErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            RpcErrorCode::Internal => write!(f, "Internal error"),
            RpcErrorCode::BadRequest => write!(f, "Bad request"),
        }
    }
}

impl From<Error> for RpcError {
    fn from(err: Error) -> Self {
        match err {
            Error::Rpc(e) => e,
            e => {
                RpcError {
                    description: error::Error::description(&e).to_string(),
                    code: RpcErrorCode::Internal,
                }
            }
        }
    }
}

impl From<pipeline::Error<Error>> for Error {
    fn from(err: pipeline::Error<Error>) -> Self {
        match err {
            pipeline::Error::Transport(e) => e,
            pipeline::Error::Io(e) => e.into(),
        }
    }
}

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;
/// Return type from server to client. Converted into ```Result<T>``` before reaching the user.
pub type RpcResult<T> = ::std::result::Result<T, RpcError>;
/// Return type from server to client. Converted into ```Result<T>``` before reaching the user.
pub type Future<T> = Box<futures::Future<Item = T, Error = RpcError>>;
