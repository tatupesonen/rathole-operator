use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("kube api error: {0}")]
    Kube(#[from] kube::Error),

    #[error("missing object field: {0}")]
    MissingField(&'static str),

    #[error("token unavailable: secret {0:?} key {1:?}")]
    TokenUnavailable(String, String),

    #[error("token secret value is not valid UTF-8")]
    TokenNotUtf8,

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("server config push failed: HTTP {0}")]
    PushFailed(u16),

    #[error("finalizer error: {0}")]
    Finalizer(#[source] Box<kube::runtime::finalizer::Error<Error>>),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Per-Service validation failure. These never fail the whole reconcile — the
/// offending Service (or port) is skipped and surfaced in the config's status.
#[derive(Debug, Error)]
#[error("service {namespace}/{name}: {reason}")]
pub struct ServiceError {
    pub namespace: String,
    pub name: String,
    pub reason: String,
}
