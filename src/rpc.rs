use serde::{Deserialize, Serialize};

use crate::embeddings::ResponseChunk;

#[tarpc::service]
pub trait CodeSearchRpc {
    async fn search_code(
        project_path: String,
        query: String,
    ) -> Result<Vec<ResponseChunk>, RpcError>;
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RpcError {
    Internal,
}
