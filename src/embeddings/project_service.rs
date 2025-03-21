use anyhow::Result;
use futures::{StreamExt, executor::block_on};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tarpc::{
    client, context,
    server::{self, Channel},
};

use crate::rpc::RpcError;

use super::{
    project_files::{ProjectFiles, ResponseChunk},
    project_repository::ProjectRepository,
};

pub struct ProjectService {
    files: ProjectFiles,
    repository: ProjectRepository,
}

impl ProjectService {
    pub fn start(path: PathBuf) -> ProjectRpcClient {
        let (client_transport, server_transport) = tarpc::transport::channel::unbounded();
        let server = server::BaseChannel::with_defaults(server_transport);
        tokio::task::spawn_blocking(move || {
            let project_service = Arc::new(Mutex::new(ProjectService::new(path).unwrap()));
            block_on(
                server
                    .execute(project_service.serve())
                    // Handle all requests sequentially.
                    .for_each(|response| response),
            )
        });
        ProjectRpcClient::new(client::Config::default(), client_transport).spawn()
    }

    fn new(path: PathBuf) -> Result<Self> {
        let files = ProjectFiles::new(path)?;
        let repository = ProjectRepository::new()?;

        for (path, chunks) in files.all_chunks() {
            repository.insert_file(&path.to_string_lossy(), chunks)?;
        }

        Ok(Self { files, repository })
    }
}

type SearchCodeResponse = Vec<ResponseChunk>;

#[tarpc::service]
pub trait ProjectRpc {
    async fn search_code(query: String) -> Result<SearchCodeResponse, RpcError>;

    async fn file_updated(path: PathBuf) -> Result<(), RpcError>;
}

impl ProjectRpc for Arc<Mutex<ProjectService>> {
    async fn search_code(
        self,
        _ctx: context::Context,
        query: String,
    ) -> Result<SearchCodeResponse, RpcError> {
        let service = self.lock().unwrap();
        let chunks = service.repository.search(&query).unwrap();
        Ok(service.files.chunks_to_response(chunks))
    }

    async fn file_updated(
        self,
        _ctx: context::Context,
        file_path: PathBuf,
    ) -> Result<(), RpcError> {
        let mut service = self.lock().unwrap();
        service.files.create_or_update(&file_path)?;
        let chunks = service.files.file_chunks(&file_path);
        service
            .repository
            .insert_file(&file_path.to_string_lossy(), chunks)?;
        Ok(())
    }
}
