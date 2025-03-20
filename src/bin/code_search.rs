use anyhow::Result;
use mcp_attr::{
    ErrorCode,
    server::{McpServer, mcp_server, serve_stdio},
};
use tarpc::{client, context, tokio_serde::formats::Json};

use local_code_search::{embeddings::ResponseChunk, rpc::*};

struct CodeSearchServer;

impl CodeSearchServer {
    async fn handle_search_code(
        &self,
        project_root: String,
        query: String,
    ) -> mcp_attr::Result<Vec<ResponseChunk>> {
        let mut transport =
            tarpc::serde_transport::unix::connect("/tmp/code_search.sock", Json::default);
        transport.config_mut().max_frame_length(usize::MAX);
        let client = CodeSearchRpcClient::new(client::Config::default(), transport.await?).spawn();
        let result = match client
            .search_code(context::current(), project_root, query)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                return Err(mcp_attr::Error::new(ErrorCode::INTERNAL_ERROR)
                    .with_message(e.to_string(), true));
            }
        };
        Ok(result.unwrap_or_default())
    }
}

#[mcp_server]
impl McpServer for CodeSearchServer {
    /// Search for code in the given project.
    #[tool]
    async fn search_code(
        &self,
        /// The root path of the project to search.
        project_root: String,
        /// The query to search for.
        query: String,
    ) -> mcp_attr::Result<Vec<String>> {
        match self.handle_search_code(project_root, query).await {
            Ok(result) => Ok(result
                .into_iter()
                .map(|chunk| {
                    format!(
                        "file://{}:{}:{}-{}:{} contains:\n{}",
                        chunk.path.to_string_lossy(),
                        chunk.row.start,
                        chunk.column.start,
                        chunk.row.end,
                        chunk.column.end,
                        chunk.content
                    )
                })
                .collect()),
            Err(e) => {
                std::fs::write(
                    "/Users/katis/code/local-code-search/code_search.log",
                    format!("error: {e:?}"),
                )
                .unwrap();
                Err(e)
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    serve_stdio(CodeSearchServer).await?;
    Ok(())
}
