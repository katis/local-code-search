use std::{ops::Range, path::PathBuf};

use anyhow::Result;
use fastembed::TextEmbedding;
use rusqlite::{Connection, OptionalExtension, params};
use zerocopy::IntoBytes;

use super::code_splitter::Chunk;

pub struct ProjectRepository {
    conn: Connection,
    model: TextEmbedding,
}

impl ProjectRepository {
    pub fn new() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let model = TextEmbedding::try_new(Default::default())?;
        conn.execute(
            "
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
            [],
        )?;
        conn.execute(
            "
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks using vec0(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL,
                start_row INTEGER NOT NULL,
                start_column INTEGER NOT NULL,
                end_row INTEGER NOT NULL,
                end_column INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                embeddings float[384]
            )",
            [],
        )?;
        Ok(Self { conn, model })
    }

    pub fn insert_file(&self, path: &str, chunks: Vec<Chunk>) -> Result<i64> {
        let file_id = match self
            .conn
            .query_row(
                "SELECT id FROM files WHERE path = ? LIMIT 1",
                [path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            Some(prev_file_id) => {
                self.conn
                    .execute("DELETE FROM chunks WHERE file_id = ?", [prev_file_id])?;
                self.conn.execute(
                    "UPDATE files SET updated_at = CURRENT_TIMESTAMP WHERE id = ?",
                    [prev_file_id],
                )?;
                prev_file_id
            }
            None => self.conn.query_row(
                "INSERT INTO files (path) VALUES (?) RETURNING id",
                [path],
                |row| row.get(0),
            )?,
        };

        let mut stmt = self.conn.prepare(
            "INSERT INTO chunks (
                file_id,
                start_row,
                start_column,
                end_row,
                end_column,
                start_byte,
                end_byte,
                embeddings
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        let content: Vec<&str> = chunks.iter().map(|chunk| chunk.text).collect();
        let embeddings = self.model.embed(content, None)?;
        for (chunk, embedding) in chunks.iter().zip(embeddings) {
            stmt.execute(params![
                file_id,
                chunk.start.row,
                chunk.start.column,
                chunk.end.row,
                chunk.end.column,
                chunk.range.start,
                chunk.range.end,
                embedding.as_bytes(),
            ])?;
        }
        Ok(file_id)
    }

    pub fn search(&self, query: &str) -> Result<Vec<OutputChunk>> {
        let query_embedding = self.model.embed(vec![query], None)?;
        let mut search_stmt = self.conn.prepare(
            "SELECT
                file_id,
                start_row,
                end_row,
                start_column,
                end_column,
                start_byte,
                end_byte,
                distance
            FROM chunks
            WHERE embeddings MATCH ?
            ORDER BY distance
            LIMIT 5",
        )?;

        let mut rows = search_stmt.query(params![query_embedding[0].as_bytes()])?;
        let mut chunks = Vec::new();
        while let Some(row) = rows.next()? {
            let file_id: i64 = row.get(0)?;
            let path =
                self.conn
                    .query_row("SELECT path FROM files WHERE id = ?", [file_id], |row| {
                        row.get::<_, String>(0)
                    })?;

            chunks.push(OutputChunk {
                path: PathBuf::from(path),
                row: row.get(1)?..row.get(2)?,
                column: row.get(3)?..row.get(4)?,
                byte: row.get(5)?..row.get(6)?,
            });
        }
        Ok(chunks)
    }
}

pub struct OutputChunk {
    pub path: PathBuf,
    pub row: Range<usize>,
    pub column: Range<usize>,
    pub byte: Range<usize>,
}
