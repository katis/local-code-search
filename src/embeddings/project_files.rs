use anyhow::Result;
use blake2::{Blake2b512, Digest};
use ignore::Walk;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
};
use tree_sitter::{Language, Tree};

use super::project_repository::{Chunk, OutputChunk};

pub struct ProjectFiles {
    files: HashMap<PathBuf, ProjectFile>,
}

impl ProjectFiles {
    pub fn new(path: PathBuf) -> Result<Self> {
        let mut files = HashMap::new();
        let supported_extensions =
            HashSet::from(["rs", "ts", "tsx", "py", "java", "kt", "json", "yaml", "yml"]);
        for result in Walk::new(path) {
            let entry = result?;
            if entry.path().is_file()
                && supported_extensions.contains(
                    &entry
                        .path()
                        .extension()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or_default(),
                )
            {
                let path_buf: PathBuf = entry.path().into();
                let file = ProjectFile::new(path_buf.clone())?;
                files.insert(path_buf, file);
            }
        }
        Ok(Self { files })
    }

    pub fn update_file(&mut self, file_path: PathBuf) -> Result<()> {
        let Some(file) = self.files.get_mut(&file_path) else {
            return Err(anyhow::anyhow!("File not found: {:?}", file_path));
        };
        file.update()?;
        Ok(())
    }

    pub fn file_chunks(&self) -> Result<Vec<(PathBuf, Vec<Chunk>)>> {
        self.files
            .iter()
            .map(|(path, file)| Ok((path.clone(), file.chunks()?)))
            .collect()
    }

    pub fn chunks_to_response(&self, chunks: Vec<OutputChunk>) -> Result<Vec<ResponseChunk>> {
        Ok(chunks
            .into_iter()
            .flat_map(|chunk| {
                let file = self.files.get(&chunk.path)?;
                Some(ResponseChunk {
                    content: file.text[chunk.byte.start..chunk.byte.end].into(),
                    path: chunk.path,
                    row: chunk.row,
                    column: chunk.column,
                })
            })
            .collect())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseChunk {
    pub path: PathBuf,
    pub row: Range<usize>,
    pub column: Range<usize>,
    pub content: String,
}

struct ProjectFile {
    parser: tree_sitter::Parser,
    language: tree_sitter::Language,
    path: String,
    text: String,
    hash: Vec<u8>,
    tree: Tree,
}

impl ProjectFile {
    pub fn new(path: PathBuf) -> Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        let Some(language) = ext_to_language(
            path.extension()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default(),
        ) else {
            return Err(anyhow::anyhow!("Unsupported file extension {:?}", path));
        };
        parser.set_language(&language)?;

        let text = std::fs::read_to_string(&path)?;
        let Some(tree) = parser.parse(&text, None) else {
            return Err(anyhow::anyhow!("Failed to parse {:?}", path));
        };
        let hash = hash_file(&text);
        Ok(Self {
            parser,
            language,
            path: path.to_string_lossy().to_string(),
            text,
            hash,
            tree,
        })
    }

    pub fn update(&mut self) -> Result<()> {
        let file_contents = std::fs::read_to_string(&self.path)?;
        let Some(new_tree) = self.parser.parse(&file_contents, Some(&self.tree)) else {
            return Err(anyhow::anyhow!("Failed to parse {:?}", self.path));
        };
        self.hash = hash_file(&file_contents);
        self.tree = new_tree;
        Ok(())
    }

    pub fn chunks(&self) -> Result<Vec<Chunk>> {
        let mut chunks = Vec::new();
        for child in self.tree.root_node().children(&mut self.tree.walk()) {
            let content = self.text[child.start_byte()..child.end_byte()].into();
            chunks.push(Chunk {
                row: child.start_position().row..child.end_position().row,
                column: child.start_position().column..child.end_position().column,
                byte: child.start_byte()..child.end_byte(),
                content,
            });
        }
        Ok(chunks)
    }
}

fn ext_to_language(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "kt" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        "json" => Some(tree_sitter_json::LANGUAGE.into()),
        "yaml" | "yml" => Some(tree_sitter_yaml::LANGUAGE.into()),
        _ => None,
    }
}

fn hash_file(content: &str) -> Vec<u8> {
    let mut hasher = Blake2b512::new();
    hasher.update(content.as_bytes());
    hasher.finalize().to_vec()
}
