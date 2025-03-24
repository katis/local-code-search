use std::ops::Range;

use recursive::recursive;
use tree_sitter::*;

pub struct CodeSplitter<'a> {
    tree: &'a Tree,
    source: &'a str,
    max_chunk_size: usize,
}

impl<'a> CodeSplitter<'a> {
    pub fn new(tree: &'a Tree, source: &'a str, max_chunk_size: usize) -> Self {
        Self {
            tree,
            source,
            max_chunk_size,
        }
    }

    pub fn chunks(&self) -> Vec<Chunk<'a>> {
        let mut chunks = Vec::new();
        let node = self.tree.root_node();
        self.process_chunks(&mut chunks, Chunk::default(), node);
        chunks
    }

    #[recursive]
    fn process_chunks<'c>(&self, chunks: &'c mut Vec<Chunk<'a>>, mut last: Chunk<'a>, node: Node) {
        let mut current_chunk: Option<Chunk> = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.byte_range().len() > self.max_chunk_size {
                if let Some(chunk) = current_chunk.take() {
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    }
                }
                self.process_chunks(chunks, last.clone(), child);
            } else if current_chunk
                .as_ref()
                .map(|chunk| self.chunk_size(chunk.range.clone()))
                .unwrap_or(0)
                + self.chunk_size(child.byte_range())
                > self.max_chunk_size
            {
                if let Some(chunk) = current_chunk.take() {
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    }
                }
                current_chunk =
                    Some(last.merge_end(Chunk::from_node(self.source, &child), self.source));
            } else {
                let new_chunk = last.merge_end(Chunk::from_node(self.source, &child), self.source);
                match current_chunk {
                    Some(chunk) if !chunk.is_empty() => {
                        current_chunk = Some(chunk.merge(new_chunk, self.source))
                    }
                    _ => current_chunk = Some(new_chunk),
                }
            }
            last = Chunk::from_node(self.source, &child);
        }
        if let Some(chunk) = current_chunk {
            if !chunk.is_empty() {
                chunks.push(chunk);
            }
        }
    }

    fn chunk_size(&self, range: Range<usize>) -> usize {
        self.source[range].chars().count()
    }
}

#[derive(Debug, Default, Clone)]
pub struct Chunk<'a> {
    pub text: &'a str,
    pub range: Range<usize>,
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Default, Clone, Copy, Eq, Ord, PartialOrd, PartialEq)]
pub struct TextPosition {
    pub row: usize,
    pub column: usize,
}

impl From<tree_sitter::Point> for TextPosition {
    fn from(point: tree_sitter::Point) -> Self {
        Self {
            row: point.row,
            column: point.column,
        }
    }
}

impl<'a> Chunk<'a> {
    fn from_node(source: &'a str, node: &Node) -> Self {
        let range = node.range();
        Self {
            text: &source[range.start_byte..range.end_byte],
            range: range.start_byte..range.end_byte,
            start: range.start_point.into(),
            end: range.end_point.into(),
        }
    }

    fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    fn merge(&self, other: Self, source: &'a str) -> Self {
        let range = self.range.start..other.range.end;
        Self {
            text: &source[range.clone()],
            range,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    fn merge_end(&self, other: Self, source: &'a str) -> Self {
        let range = self.range.end..other.range.end;
        Self {
            text: &source[range.clone()],
            range,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_splitter() {
        let code = std::fs::read_to_string("src/embedings/project_repository.rs").unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&code, None).unwrap();
        let splitter = CodeSplitter::new(&tree, &code, 1500);
        let chunks = splitter.chunks();
        let text = chunks
            .iter()
            .enumerate()
            .fold(String::new(), |acc, (i, c)| {
                format!("{}\n====CHUNK {}=====\n{}", acc, i, c.text)
            });
        println!("{}", text);
        assert_eq!(text, "\nfn main()\n\n{\nlet x = 1;\nlet y = 2;\n}");
    }
}
