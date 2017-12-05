/// TLS-specific functionality for clients.
pub mod client {
    use native_tls::{Error, TlsConnector};
    use std::fmt;

    /// TLS context for client
    pub struct Context {
        /// Domain to connect to
        pub domain: String,
        /// TLS connector
        pub tls_connector: TlsConnector,
    }

    impl Context {
        /// Try to construct a new `Context`.
        ///
        /// The provided domain will be used for both
        /// [SNI](https://en.wikipedia.org/wiki/Server_Name_Indication) and certificate hostname
        /// validation.
        pub fn new<S: Into<String>>(domain: S) -> Result<Self, Error> {
            Ok(Context {
                domain: domain.into(),
                tls_connector: TlsConnector::builder()?.build()?,
            })
        }

        /// Construct a new `Context` using the provided domain and `TlsConnector`
        ///
        /// The domain will be used for both
        /// [SNI](https://en.wikipedia.org/wiki/Server_Name_Indication) and certificate hostname
        /// validation.
        pub fn from_connector<S: Into<String>>(domain: S, tls_connector: TlsConnector) -> Self {
            Context {
                domain: domain.into(),
                tls_connector: tls_connector,
            }
        }
    }

    impl fmt::Debug for Context {
        fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
            const TLS_CONNECTOR: &str = "TlsConnector { .. }";
            f.debug_struct("Context")
                .field("domain", &self.domain)
                .field("tls_connector", &TLS_CONNECTOR)
                .finish()
        }
    }

}
