use anyhow::Result;
use dashmap::{DashMap, mapref::one::RefMut};
use futures::{future, prelude::*};
use rusqlite::ffi::sqlite3_auto_extension;
use sqlite_vec::sqlite3_vec_init;
use std::{path::PathBuf, sync::Arc};
use tarpc::{
    context,
    server::{self, Channel},
    tokio_serde::formats::Json,
};

use local_code_search::{
    embeddings::{ProjectRpcClient, ProjectService, ResponseChunk},
    rpc::*,
};

#[derive(Clone)]
struct CodeSearchServer(Arc<DashMap<PathBuf, ProjectRpcClient>>);

impl CodeSearchRpc for CodeSearchServer {
    async fn search_code(
        self,
        _: context::Context,
        project_path: String,
        query: String,
    ) -> Result<Vec<ResponseChunk>, RpcError> {
        let project_path = std::fs::canonicalize(project_path).unwrap();
        let project_stub = self.project_stub(project_path);
        let response = project_stub.search_code(context::current(), query).await?;
        response
    }
}

impl CodeSearchServer {
    fn project_stub(&self, project_path: PathBuf) -> RefMut<'_, PathBuf, ProjectRpcClient> {
        self.0
            .entry(project_path.clone())
            .or_insert_with(|| ProjectService::start(project_path))
    }
}

#[actix::main]
async fn main() -> Result<()> {
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }

    let socket_path = "/tmp/code_search.sock";
    std::fs::remove_file(socket_path).ok();

    let server = CodeSearchServer(Arc::default());
    let mut listener = tarpc::serde_transport::unix::listen(socket_path, Json::default).await?;
    listener.config_mut().max_frame_length(usize::MAX);
    listener
        .filter_map(|r| future::ready(r.ok()))
        .map(server::BaseChannel::with_defaults)
        .map(move |channel| channel.execute(server.clone().serve()).for_each(spawn))
        // Max 10 channels.
        .buffer_unordered(10)
        .for_each(|_| async {})
        .await;

    Ok(())
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}
