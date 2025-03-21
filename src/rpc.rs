use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::embeddings::ResponseChunk;

#[tarpc::service]
pub trait CodeSearchRpc {
    async fn search_code(
        project_path: String,
        query: String,
    ) -> Result<Vec<ResponseChunk>, RpcError>;
}

#[derive(Debug, Serialize, Deserialize, Error)]
pub enum RpcError {
    #[error("Tarpc error: {0}")]
    Tarpc(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for RpcError {
    fn from(error: anyhow::Error) -> Self {
        RpcError::Internal(error.to_string())
    }
}

impl From<tarpc::client::RpcError> for RpcError {
    fn from(error: tarpc::client::RpcError) -> Self {
        RpcError::Tarpc(error.to_string())
    }
}
