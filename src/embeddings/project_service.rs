use anyhow::Result;
use std::{
    path::PathBuf,
    thread::{JoinHandle, spawn},
};

use tokio::sync::{mpsc, oneshot};

use super::{
    project_files::{ProjectFiles, ResponseChunk},
    project_repository::ProjectRepository,
};

pub struct ProjectService {
    files: ProjectFiles,
    repository: ProjectRepository,
}

impl ProjectService {
    pub fn start(path: PathBuf) -> ProjectStub {
        let (tx, mut rx) = mpsc::channel(1);
        let handle = spawn(move || {
            let mut service = ProjectService::new(path).unwrap();
            while let Some(message) = rx.blocking_recv() {
                if let Err(e) = service.receive(message) {
                    eprintln!("Error: {}", e);
                }
            }
        });
        ProjectStub {
            tx,
            _handle: handle,
        }
    }

    fn new(path: PathBuf) -> Result<Self> {
        let files = ProjectFiles::new(path)?;
        let repository = ProjectRepository::new()?;

        for (path, chunks) in files.file_chunks()? {
            repository.insert_file(&path.to_string_lossy(), chunks)?;
        }

        Ok(Self { files, repository })
    }

    fn receive(&mut self, message: Message) -> Result<()> {
        match message {
            Message::SearchCode(query, respond) => {
                let response = self.search_code(query);
                respond.send(response).unwrap();
            }
        }
        Ok(())
    }

    fn search_code(&mut self, query: String) -> Result<SearchCodeResponse> {
        let chunks = self.repository.search(&query)?;
        let response = self.files.chunks_to_response(chunks)?;
        Ok(response)
    }
}

type SearchCodeResponse = Vec<ResponseChunk>;

pub enum Message {
    SearchCode(String, oneshot::Sender<Result<SearchCodeResponse>>),
}

pub struct ProjectStub {
    tx: mpsc::Sender<Message>,
    _handle: JoinHandle<()>,
}

impl ProjectStub {
    pub async fn search_code(&self, query: String) -> Result<SearchCodeResponse> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Message::SearchCode(query, tx)).await?;
        let result = rx.await?;
        result
    }
}
