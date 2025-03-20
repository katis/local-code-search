use anyhow::Result;
use dashmap::{DashMap, mapref::one::RefMut};
use futures::{future, prelude::*};
use ignore::Walk;
use rusqlite::ffi::sqlite3_auto_extension;
use sqlite_vec::sqlite3_vec_init;
use std::{
    collections::HashSet,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};
use tarpc::{
    context,
    server::{self, Channel},
    tokio_serde::formats::Json,
};
use tree_sitter::Language;

use local_code_search::{
    embeddings::{ProjectService, ProjectStub, ResponseChunk},
    rpc::*,
};

#[derive(Clone)]
struct CodeSearchServer(Arc<DashMap<PathBuf, ProjectStub>>);

impl CodeSearchRpc for CodeSearchServer {
    async fn search_code(
        self,
        _: context::Context,
        project_path: String,
        query: String,
    ) -> Result<Vec<ResponseChunk>, RpcError> {
        let project_path = std::fs::canonicalize(project_path).unwrap();
        let project_stub = self.project_stub(project_path);
        let response = project_stub.search_code(query).await.map_err(|e| {
            println!("error: {e:?}");
            RpcError::Internal
        })?;
        Ok(response)
    }
}

impl CodeSearchServer {
    fn project_stub(&self, project_path: PathBuf) -> RefMut<'_, PathBuf, ProjectStub> {
        self.0
            .entry(project_path.clone())
            .or_insert_with(|| ProjectService::start(project_path))
    }
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}

#[tokio::main]
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

    // // Remove the database file if it exists, for testing
    // std::fs::remove_file("./code_search.sqlite").ok();

    // let db = Connection::open("./code_search.sqlite")?;
    // db.execute(
    //     "
    //     create table code_files (
    //         id INTEGER PRIMARY KEY,
    //         path TEXT NOT NULL
    //     );",
    //     [],
    // )?;
    // db.execute(
    //     "
    //     create virtual table code_file_content using vec0(
    //         id  INTEGER PRIMARY KEY,
    //         code_file_id INT NOT NULL,
    //         start_row INT NOT NULL,
    //         end_row INT NOT NULL,
    //         content float[384]
    //     );",
    //     [],
    // )?;

    // let supported_extensions = HashSet::from(["rs", "ts", "tsx", "py", "java", "kt"]);
    // let files = find_code_files_in(&args.path, &supported_extensions)?;

    // let model = TextEmbedding::try_new(Default::default())?;
    // let mut insert_file = db.prepare("insert into code_files (path) values (?) RETURNING id")?;
    // let mut insert_file_content = db.prepare(
    //     "insert into code_file_content (code_file_id, start_row, end_row, content) values (?, ?, ?, ?)",
    // )?;
    // for file in &files {
    //     let text = std::fs::read_to_string(file)?;
    //     let (line_numbers, chunks) = split_code_into_chunks(file, &text)?;
    //     let embedding = model.embed(chunks, None)?;
    //     let file_id: i64 = insert_file
    //         .query_row(params![file.to_string_lossy().into_owned()], |row| {
    //             row.get(0)
    //         })?;
    //     for (line_number, chunk) in line_numbers.iter().zip(embedding) {
    //         insert_file_content.execute(params![
    //             file_id,
    //             line_number.start,
    //             line_number.end,
    //             chunk.as_bytes()
    //         ])?;
    //     }
    // }

    // let mut query_code = db.prepare(
    //     "SELECT code_file_id, start_row, end_row, distance
    //     FROM code_file_content
    //     WHERE content MATCH ?
    //     ORDER BY distance
    //     LIMIT 10",
    // )?;
    // let mut query_file_name = db.prepare("SELECT path FROM code_files WHERE id = ?")?;

    // let content = model.embed(vec!["split chunks"], None)?;
    // let mut rows = query_code.query(params![content[0].as_bytes()])?;
    // while let Some(row) = rows.next()? {
    //     let code_file_id: i64 = row.get(0)?;
    //     let start_row: i64 = row.get(1)?;
    //     let end_row: i64 = row.get(2)?;
    //     let file_name: String =
    //         query_file_name.query_row(params![code_file_id], |row| row.get(0))?;
    //     let distance: f32 = row.get(2)?;
    //     println!("{file_name:?}:{start_row}..{end_row} ({distance})");
    // }

    Ok(())
}

fn split_code_into_chunks(path: &Path, text: &str) -> Result<(Vec<Range<usize>>, Vec<String>)> {
    let ext = path
        .extension()
        .map(|ext| ext.to_str().unwrap_or_default())
        .unwrap_or_default();
    let Some(language) = ext_to_language(ext) else {
        return Err(anyhow::anyhow!("Unsupported file extension {path:?}"));
    };
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language)?;
    let Some(tree) = parser.parse(text, None) else {
        return Err(anyhow::anyhow!("Failed to parse {path:?}"));
    };
    let root_node = tree.root_node();
    let mut chunks = Vec::new();
    let mut line_numbers = Vec::new();
    for child in root_node.children(&mut tree.walk()) {
        let start = child.start_byte();
        let end = child.end_byte();
        let content = text[start..end].to_string();
        chunks.push(content.into());
        line_numbers.push(child.start_position().row..child.end_position().row);
    }
    Ok((line_numbers, chunks))
}

fn find_code_files_in(path: &str, supported_extensions: &HashSet<&str>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for result in Walk::new(path) {
        let entry = result?;
        if entry.path().is_file() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if supported_extensions.contains(&ext) {
                    if let Some(_) = entry.path().to_str() {
                        // Try to get the relative path by stripping the base directory
                        let relative_path = match entry.path().strip_prefix(path) {
                            Ok(rel_path) => rel_path,
                            Err(e) => {
                                println!("Failed to strip prefix: {}", e);
                                // Fall back to full path if stripping fails
                                entry.path()
                            }
                        };
                        files.push(relative_path.to_path_buf());
                    }
                }
            }
        }
    }
    Ok(files)
}

fn ext_to_language(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "kt" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        _ => None,
    }
}
