use anyhow::Result;
use dashmap::{DashMap, mapref::one::RefMut};
use futures::{future, prelude::*};
use ignore_files::IgnoreFilter;
use rusqlite::ffi::sqlite3_auto_extension;
use sqlite_vec::sqlite3_vec_init;
use std::{path::PathBuf, sync::Arc};
use tarpc::{
    context,
    server::{self, Channel},
    tokio_serde::formats::Json,
};
use tokio::sync::mpsc;
use watchexec::{WatchedPath, Watchexec, filter::Filterer};
use watchexec_events::{Event, FileType, Tag, filekind::FileEventKind};
use watchexec_filterer_ignore::IgnoreFilterer;
use watchexec_signals::Signal;

use local_code_search::{
    embeddings::{ProjectRpcClient, ProjectService, ResponseChunk},
    rpc::*,
};

#[derive(Clone)]
struct CodeSearchServer(Arc<CodeSearchServerState>);

impl CodeSearchRpc for CodeSearchServer {
    async fn search_code(
        self,
        _: context::Context,
        project_path: String,
        query: String,
    ) -> Result<Vec<ResponseChunk>, RpcError> {
        let project_path = std::fs::canonicalize(project_path).unwrap();
        let project_stub = self.project_rpc(project_path).await;
        let response = project_stub.search_code(context::current(), query).await?;
        response
    }
}

impl CodeSearchServer {
    pub fn file_created_or_modified(&self, path: PathBuf) {
        println!("file_created_or_modified: {:?}", path);
        // let project_stub = self.project_rpc(path.parent().unwrap().to_path_buf());
        // project_stub.file_updated(context::current(), path);
    }

    // pub fn file_deleted(&self, path: PathBuf) {
    //     let project_stub = self.project_rpc(path.parent().unwrap().to_path_buf());
    //     project_stub.file_deleted(context::current(), path);
    // }

    // fn project_of_file(&self, file_path: PathBuf) -> ProjectRpcClient {}

    async fn project_rpc(&self, project_path: PathBuf) -> RefMut<'_, PathBuf, ProjectRpcClient> {
        match self.0.projects.get_mut(&project_path) {
            Some(project) => project,
            None => {
                let mut paths = self.0.watch_config.pathset.get();
                paths.push(WatchedPath::recursive(project_path.clone()));
                self.0.watch_config.pathset(paths);

                self.0.filter_path.send(project_path.clone()).await;

                let project = ProjectService::start(project_path.clone());
                self.0.projects.insert(project_path.clone(), project);
                self.0.projects.get_mut(&project_path).unwrap()
            }
        }
    }
}

#[derive(Debug, Clone)]
struct MultiFilterer<F> {
    filterers: Vec<F>,
}

impl<F: Send> MultiFilterer<F> {
    fn new(filterers: Vec<F>) -> Self {
        Self { filterers }
    }

    fn add(&mut self, filterer: F) {
        self.filterers.push(filterer);
    }
}

impl MultiFilterer<IgnoreFilterer> {
    async fn ignore_from_origin(
        &mut self,
        path: &std::path::Path,
    ) -> anyhow::Result<(), watchexec::error::RuntimeError> {
        let (ignored, errors) = ignore_files::from_origin(path).await;
        if !errors.is_empty() {
            println!("errors: {:?}", errors);
        }
        let filterer = IgnoreFilter::new(path, &ignored).await?;
        self.add(IgnoreFilterer(filterer));
        Ok(())
    }
}

impl<F: Filterer> Filterer for MultiFilterer<F> {
    fn check_event(
        &self,
        event: &Event,
        priority: watchexec_events::Priority,
    ) -> Result<bool, watchexec::error::RuntimeError> {
        for filterer in self.filterers.iter() {
            if !filterer.check_event(event, priority)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

struct CodeSearchServerState {
    watch_config: watchexec::Config,
    projects: DashMap<PathBuf, ProjectRpcClient>,
    filter_path: mpsc::Sender<PathBuf>,
}

#[actix::main]
async fn main() -> Result<()> {
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }

    let socket_path = "/tmp/code_search.sock";
    std::fs::remove_file(socket_path).ok();

    let config = watchexec::Config::default();
    let (add_project_path_tx, mut add_project_path_rx) = tokio::sync::mpsc::channel::<PathBuf>(10);
    let server = CodeSearchServer(Arc::new(CodeSearchServerState {
        watch_config: config.clone(),
        projects: DashMap::new(),
        filter_path: add_project_path_tx,
    }));

    config.on_action({
        let server = server.clone();
        move |mut action| {
            for event in action.events.iter() {
                println!("EVENTTI: {:?}", event);
                if event.tags.iter().any(|tag| {
                    matches!(
                        tag,
                        Tag::FileEventKind(FileEventKind::Create(_) | FileEventKind::Modify(_))
                    )
                }) {
                    for (path, file_type) in event.paths() {
                        match file_type {
                            Some(FileType::File) => {
                                server.file_created_or_modified(path.to_path_buf());
                            }
                            _ => {}
                        }
                    }
                }
            }
            if action.signals().any(|sig| {
                matches!(
                    sig,
                    Signal::Interrupt | Signal::Terminate | Signal::ForceStop
                )
            }) {
                action.quit();
            }
            action
        }
    });
    let wx = Arc::new(Watchexec::with_config(config).unwrap());

    let mut listener = tarpc::serde_transport::unix::listen(socket_path, Json::default).await?;
    listener.config_mut().max_frame_length(usize::MAX);
    tokio::spawn({
        let server = server.clone();
        async move {
            listener
                .filter_map(|r| future::ready(r.ok()))
                .map(server::BaseChannel::with_defaults)
                .map(move |channel| {
                    println!("NEW CHANNEL");
                    channel.execute(server.clone().serve()).for_each(spawn)
                })
                // Max 10 channels.
                .buffer_unordered(10)
                .for_each(|_| async {})
                .await;
        }
    });

    tokio::task::spawn_local({
        let wx = wx.clone();
        async move {
            // the internals of ignore_files are not Send, so we spawn a task in
            // the current thread and communicate with the Server task via a channel
            let mut filterers = MultiFilterer::new(vec![]);
            while let Some(path) = add_project_path_rx.recv().await {
                if let Err(e) = filterers.ignore_from_origin(&path).await {
                    println!("error: {:?}", e);
                }
            }
            wx.config.filterer(filterers);
        }
    });

    wx.main().await??;
    println!("Watchexec exited");

    Ok(())
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}
