// highlight.rs: syntax highlighting through tree-sitter.
//
// Still no winit, no glow, no font: this module produces *spans* (character
// ranges with a `TokenKind`) and the ui picks the colours. That keeps the theme
// in one place and lets this be tested without a window.
//
// Lazy by design (v1.2 section 6): the `Language`, `Parser` and `Query` for a
// language are built the first time a file of that type is opened, never at
// startup, so cold start pays nothing.
//
// Only the visible lines are queried. The query cursor is given a byte range,
// and the resulting spans are cached until the buffer version or the visible
// range changes, so a cursor blink or a hover redraw never re-runs the query.

use std::path::Path;
use std::sync::OnceLock;

use tree_sitter::Tree;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, StreamingIterator};

use crate::editor::buffer::Buffer;

#[cfg(test)]
mod tests;

/// Files above this size are not parsed at all: the status bar says so and
/// editing stays fast (v1.2 section 6).
pub const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;
/// Above this an initial parse goes to a background thread instead of the frame.
pub const BIG_PARSE_BYTES: usize = 256 * 1024;

/// The languages we highlight, chosen by file extension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    C,
    Cpp,
    Python,
    Json,
    Toml,
    Markdown,
}

impl Lang {
    /// Pick a language from a path's extension. `None` means "no highlighting",
    /// which is a normal state, not an error.
    pub fn for_path(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Lang::Rust,
            "c" | "h" => Lang::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            "py" | "pyi" => Lang::Python,
            "json" => Lang::Json,
            "toml" => Lang::Toml,
            "md" | "markdown" => Lang::Markdown,
            _ => return None,
        })
    }

    /// The tree-sitter language and the highlight query for it.
    ///
    /// Building the `Language` and compiling the query is not free (~1 ms), so
    /// each one is created once per process and shared through a `OnceLock`.
    fn grammar(self) -> (Language, QuerySource) {
        match self {
            Lang::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                QuerySource::Single(tree_sitter_rust::HIGHLIGHTS_QUERY),
            ),
            Lang::C => (
                tree_sitter_c::LANGUAGE.into(),
                QuerySource::Single(tree_sitter_c::HIGHLIGHT_QUERY),
            ),
            // The C++ query only *adds* C++-specific nodes (templates,
            // namespaces, `auto`), so it has to be combined with the C query:
            // without that, plain `int main()` gets no colours at all. This is
            // how the crate's README says to use it.
            Lang::Cpp => (
                tree_sitter_cpp::LANGUAGE.into(),
                QuerySource::Joined(
                    tree_sitter_c::HIGHLIGHT_QUERY,
                    tree_sitter_cpp::HIGHLIGHT_QUERY,
                ),
            ),
            Lang::Python => (
                tree_sitter_python::LANGUAGE.into(),
                QuerySource::Single(tree_sitter_python::HIGHLIGHTS_QUERY),
            ),
            Lang::Json => (
                tree_sitter_json::LANGUAGE.into(),
                QuerySource::Single(tree_sitter_json::HIGHLIGHTS_QUERY),
            ),
            Lang::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                QuerySource::Single(tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
            ),
            // v1.2 section 6: block grammar only. Inline emphasis and injected
            // code fences are explicitly out of scope.
            Lang::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                QuerySource::Single(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),
            ),
        }
    }
}

/// Where a language's highlight query comes from. C++ needs two files joined,
/// which cannot be a `&'static str` without a build script.
#[derive(Clone, Copy)]
enum QuerySource {
    Single(&'static str),
    Joined(&'static str, &'static str),
}

impl QuerySource {
    fn compile(self, language: &Language) -> Option<Query> {
        let query = match self {
            QuerySource::Single(src) => Query::new(language, src),
            QuerySource::Joined(a, b) => {
                let joined = format!("{a}\n{b}");
                Query::new(language, &joined)
            }
        };
        query.ok()
    }
}

/// The small set of colours the theme actually has. Unknown capture names map
/// to `Unknown`, which draws with the normal text colour, so a grammar update
/// can never make text unreadable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TokenKind {
    #[default]
    Unknown,
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Constant,
    Variable,
    Property,
    Operator,
    Punctuation,
    /// Markdown headings, code fence markers, list markers, quotes.
    Heading,
}

impl TokenKind {
    /// Map a tree-sitter capture name. Longest prefixes first, because grammars
    /// use dotted names (`punctuation.bracket`, `function.method`).
    pub fn from_capture(name: &str) -> TokenKind {
        // Two naming styles are in use. Most grammars lead with the family
        // (`keyword.control`, `punctuation.bracket`); the Markdown block grammar
        // leads with the node type and puts the meaning last (`text.title`).
        // Try the whole name, then the first segment, then the last one, so
        // both styles land on a colour instead of falling through to plain text.
        let head = name.split('.').next().unwrap_or(name);
        let tail = name.rsplit('.').next().unwrap_or(name);
        if let Some(kind) = Self::from_word(tail) {
            // `text.literal` is code; `text.title` is a heading. Anything whose
            // last segment is a known family wins over its first segment.
            if !matches!(head, "punctuation" | "keyword" | "string" | "comment") {
                return kind;
            }
        }
        if let Some(kind) = Self::from_word(head) {
            return kind;
        }
        // `punctuation.special` and friends: the family really is the head.
        Self::from_word(head).unwrap_or(TokenKind::Unknown)
    }

    /// One dotted segment to a kind, if it names a family we colour.
    fn from_word(word: &str) -> Option<TokenKind> {
        let kind = match word {
            "keyword" | "conditional" | "repeat" | "include" | "exception" => TokenKind::Keyword,
            "string" | "escape" | "char" | "literal" => TokenKind::String,
            "comment" => TokenKind::Comment,
            "function" | "constructor" | "method" => TokenKind::Function,
            "type" | "class" | "struct" | "enum" | "interface" => TokenKind::Type,
            "number" | "float" => TokenKind::Number,
            "constant" | "boolean" | "null" => TokenKind::Constant,
            "variable" | "parameter" => TokenKind::Variable,
            "property" | "field" | "attribute" => TokenKind::Property,
            "operator" => TokenKind::Operator,
            "punctuation" | "delimiter" | "bracket" | "special" => TokenKind::Punctuation,
            "title" | "heading" => TokenKind::Heading,
            // `text`, `label`, `embedded`, `none`, ... keep the plain text
            // colour rather than guessing.
            _ => return None,
        };
        Some(kind)
    }
}

/// One highlighted range on a line: `[start_col, end_col)` in **characters**.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start_col: u32,
    pub end_col: u32,
    pub kind: TokenKind,
}

/// The compiled query for one language, created once per process.
fn query_for(lang: Lang) -> Option<&'static Query> {
    // One slot per language: `OnceLock<Query>` inside a match would be seven
    // statics, which is clearer than a lookup table of boxed closures.
    macro_rules! slot {
        ($name:ident, $lang:expr) => {{
            static $name: OnceLock<Option<Query>> = OnceLock::new();
            $name
                .get_or_init(|| {
                    let (language, source) = $lang.grammar();
                    source.compile(&language)
                })
                .as_ref()
        }};
    }
    match lang {
        Lang::Rust => slot!(RUST, Lang::Rust),
        Lang::C => slot!(C, Lang::C),
        Lang::Cpp => slot!(CPP, Lang::Cpp),
        Lang::Python => slot!(PYTHON, Lang::Python),
        Lang::Json => slot!(JSON, Lang::Json),
        Lang::Toml => slot!(TOML, Lang::Toml),
        Lang::Markdown => slot!(MD, Lang::Markdown),
    }
}

/// A text change in both byte and point form, ready for `Tree::edit`.
///
/// `ropey` is char-indexed and tree-sitter is byte-indexed, so the constructor
/// converts with `char_to_byte`. Multi-byte text is the reason this exists as a
/// type with tests rather than a few inline sums.
#[derive(Clone, Copy, Debug)]
pub struct Edit {
    /// Character index where the change starts.
    pub start_char: usize,
    /// Characters removed at `start_char`.
    pub old_chars: usize,
    /// Characters inserted at `start_char`.
    pub new_chars: usize,
}

impl Edit {
    /// Turn a char-indexed edit into a `tree_sitter::InputEdit` for `buffer`.
    ///
    /// Returns `None` when the range is out of bounds, which only happens if a
    /// caller passes a stale edit; refusing is safer than panicking mid-frame.
    pub fn input_edit(self, buffer: &Buffer) -> Option<InputEdit> {
        let len = buffer.rope.len_chars();
        let start = self.start_char.min(len);
        let old_end_char = start.saturating_add(self.old_chars).min(len);
        let new_end_char = start.saturating_add(self.new_chars).min(len);

        let start_byte = char_to_byte(buffer, start);
        let old_end_byte = char_to_byte(buffer, old_end_char);
        let new_end_byte = char_to_byte(buffer, new_end_char);

        let point = |c: usize| {
            let (line, _col) = buffer.line_col_of(c);
            // Point columns are bytes within the line, not characters.
            let line_start = buffer.rope.line_to_char(line);
            let col_bytes = char_to_byte(buffer, c) - char_to_byte(buffer, line_start);
            Point::new(line, col_bytes)
        };

        Some(InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: point(start),
            old_end_position: point(old_end_char),
            new_end_position: point(new_end_char),
        })
    }
}

/// Byte offset of a character index. `ropey` offers no direct conversion, so
/// this walks the line the character is on, which is O(line length).
pub fn char_to_byte(buffer: &Buffer, char_index: usize) -> usize {
    let chars = buffer.rope.len_chars();
    let c = char_index.min(chars);
    let line = buffer.rope.char_to_line(c);
    let line_start = buffer.rope.line_to_char(line);
    let byte_start = buffer.rope.char_to_byte(line_start);
    byte_start + buffer.rope.slice(line_start..c).len_bytes()
}

pub struct Highlighter {
    lang: Lang,
    query: &'static Query,
    parser: Box<Parser>,
    tree: Option<Tree>,
    /// Buffer text version the tree was parsed from.
    version: u64,
    /// The visible range the cached spans cover.
    cached_range: Option<(usize, usize)>,
    /// Spans per line, indexed from `cached_first`.
    cached_spans: Vec<Vec<Span>>,
    cached_first: usize,
    /// The buffer text the tree and query were built from. Kept because
    /// converting a 336 KB rope to a `String` costs ~1 ms, and doing that twice
    /// per keystroke (once to parse, once to query) doubled key latency
    /// (measured: 39 ms/key vs 19 ms/key on a 5,000-line file).
    src_text: String,
    src_version: u64,
    /// Version the background parser is working on, if any. While it is set,
    /// the UI keeps drawing plain text instead of blocking on a parse.
    pending_version: Option<u64>,
}

impl Highlighter {
    /// Build a highlighter for `lang`, or `None` when the query will not
    /// compile. `None` means "no colours", never a crash.
    pub fn new(lang: Lang) -> Option<Self> {
        let query = query_for(lang)?;
        let (language, _) = lang.grammar();
        let mut parser = Box::new(Parser::new());
        parser.set_language(&language).ok()?;
        Some(Self {
            lang,
            query,
            parser,
            tree: None,
            version: u64::MAX,
            cached_range: None,
            cached_spans: Vec::new(),
            cached_first: 0,
            src_text: String::new(),
            src_version: u64::MAX,
            pending_version: None,
        })
    }

    /// Apply a text change to the existing tree, so the next parse only
    /// re-reads the affected part.
    pub fn edit(&mut self, e: &InputEdit) {
        if let Some(tree) = self.tree.as_mut() {
            tree.edit(e);
        }
        // Any edit invalidates the cached spans.
        self.cached_range = None;
    }

    /// The whole buffer as a string, needed by tree-sitter and by the query
    /// cursor. Only called once per parse, never per frame.
    fn text(buffer: &Buffer) -> String {
        buffer.rope.to_string()
    }

    /// Parse in the background when the file is big enough that a foreground
    /// parse would drop frames (v1.2 section 6: over 256 KB).
    ///
    /// The tree is built on a thread and handed back through `EventLoopProxy`
    /// as a `UserEvent::ParseDone`, so the frame that opens the file draws plain
    /// text immediately. Returns true when a parse was started.
    ///
    /// `done` is called on the worker thread; `app.rs` turns it into a proxy
    /// send, which is why this module needs no winit.
    #[cfg(feature = "syntax")]
    pub fn reparse_async(
        &mut self,
        buffer: &Buffer,
        version: u64,
        done: impl FnOnce(u64, Option<Tree>) + Send + 'static,
    ) -> bool {
        if self.version == version && self.tree.is_some() {
            return false;
        }
        if self.pending_version == Some(version) {
            return false; // already in flight for this exact text
        }
        let (language, _) = self.lang.grammar();
        let text = Self::text(buffer);
        let old = self.tree.take();
        self.pending_version = Some(version);
        // A fresh parser on the worker: `Parser` is not `Sync`, and building one
        // costs microseconds next to the parse itself.
        std::thread::spawn(move || {
            let mut parser = Parser::new();
            let tree = match parser.set_language(&language) {
                Ok(()) => parser.parse(&text, old.as_ref()),
                Err(_) => None,
            };
            done(version, tree);
        });
        true
    }

    /// Install a tree that came back from `reparse_async`. A result for a
    /// version that is no longer current is dropped: the text moved on while
    /// the parse was running.
    pub fn finish_async(&mut self, version: u64, tree: Option<Tree>, current: u64) -> bool {
        if self.pending_version != Some(version) {
            return false;
        }
        self.pending_version = None;
        if version != current {
            // Stale: ask for another parse on the next frame.
            self.version = u64::MAX;
            return false;
        }
        self.tree = tree;
        self.version = version;
        self.cached_range = None;
        true
    }

    /// True when this buffer is big enough to parse on a thread.
    pub fn wants_async(&self, buffer: &Buffer) -> bool {
        buffer.rope.len_bytes() > BIG_PARSE_BYTES
    }

    /// Re-parse if the buffer changed. Returns true when a parse happened.
    pub fn reparse(&mut self, buffer: &Buffer, version: u64) -> bool {
        if self.version == version && self.tree.is_some() {
            return false;
        }
        // A background parse is in flight: report "nothing to draw yet" rather
        // than starting a second, blocking parse on the UI thread.
        if self.pending_version.is_some() {
            return false;
        }
        // The *first* parse of a big file is the expensive one (measured: 93 ms
        // for 336 KB). Leave it to the worker; the caller draws plain text until
        // it lands. Once a tree exists, edits re-parse incrementally and are
        // cheap enough to stay inline.
        if self.tree.is_none() && self.wants_async(buffer) {
            return false;
        }
        if self.src_version != version {
            self.src_text = Self::text(buffer);
            self.src_version = version;
        }
        let old = self.tree.take();
        self.tree = self.parser.parse(&self.src_text, old.as_ref());
        self.version = version;
        self.cached_range = None;
        true
    }

    /// Spans for lines `first..last` (exclusive), one vec per line, with column
    /// offsets in **characters** so the ui can slice a rope line directly.
    ///
    /// The query only runs over a byte range covering those lines plus one line
    /// of margin, so a 5,000-line file costs the same per frame as a 50-line one.
    pub fn spans_for_lines(
        &mut self,
        buffer: &Buffer,
        first: usize,
        last: usize,
        version: u64,
    ) -> Vec<Vec<Span>> {
        let lines = buffer.line_count();
        let first = first.min(lines.saturating_sub(1));
        let last = last.max(first + 1).min(lines);
        if let Some((cf, cl)) = self.cached_range {
            if cf == first && cl == last && self.version == version {
                return self.cached_spans.clone();
            }
        }
        if !self.reparse(buffer, version) && self.tree.is_none() {
            return vec![Vec::new(); last - first];
        }
        let Some(tree) = self.tree.as_ref() else {
            return vec![Vec::new(); last - first];
        };

        // Reuse the string the tree was parsed from instead of rebuilding it.
        if self.src_version != version {
            self.src_text = Self::text(buffer);
            self.src_version = version;
        }
        let text = std::mem::take(&mut self.src_text);
        // Byte range with one line of margin at each end: a token that starts
        // just above the viewport can still reach into it.
        let lo_line = first.saturating_sub(1);
        let hi_line = (last + 1).min(lines);
        let lo_byte = if lo_line == 0 {
            0
        } else {
            let c = buffer.rope.line_to_char(lo_line);
            char_to_byte(buffer, c)
        };
        let hi_byte = if hi_line >= lines {
            text.len()
        } else {
            let c = buffer.rope.line_to_char(hi_line);
            char_to_byte(buffer, c)
        };

        let mut spans: Vec<Vec<Span>> = vec![Vec::new(); last - first];
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(lo_byte..hi_byte);
        let mut it = cursor.matches(self.query, tree.root_node(), text.as_bytes());
        // A capture covers a byte range; convert it to line and char columns
        // only for lines we are drawing.
        while let Some(m) = it.next() {
            for cap in m.captures() {
                let name = self.query.capture_names()[cap.index as usize];
                let kind = TokenKind::from_capture(name);
                if kind == TokenKind::Unknown {
                    continue;
                }
                let node = cap.node;
                let start = node.start_position();
                let end = node.end_position();
                // Only single-line captures become spans; a multi-line string or
                // block comment would otherwise have to be split, and drawing it
                // as one plain run per line is what the editor already does.
                if start.row != end.row {
                    continue;
                }
                if start.row < first || start.row >= last {
                    continue;
                }
                let line_chars = buffer.line_len(start.row);
                let start_col = byte_col_to_char(&text, buffer, start.row, start.column);
                let end_col = byte_col_to_char(&text, buffer, start.row, end.column);
                if start_col >= end_col || start_col >= line_chars {
                    continue;
                }
                let row = start.row - first;
                spans[row].push(Span {
                    start_col: start_col as u32,
                    end_col: (end_col.min(line_chars)) as u32,
                    kind,
                });
            }
        }
        // Later patterns win in tree-sitter only for the same node; for drawing
        // we sort by start and let the later span paint over the earlier one.
        for row in spans.iter_mut() {
            row.sort_by_key(|s| (s.start_col, s.end_col));
        }
        self.cached_range = Some((first, last));
        self.cached_spans = spans.clone();
        self.cached_first = first;
        // Give the string back so the next frame reuses it.
        self.src_text = text;
        spans
    }
}

/// Byte column within `row` to a character column.
fn byte_col_to_char(text: &str, buffer: &Buffer, row: usize, byte_col: usize) -> usize {
    let line_start_char = buffer.rope.line_to_char(row);
    let line_start_byte = char_to_byte(buffer, line_start_char);
    let line_end_char = line_start_char + buffer.line_len(row);
    let line_end_byte = char_to_byte(buffer, line_end_char);
    // Slice exactly this line's bytes so the count is characters, not bytes.
    let want = (line_start_byte + byte_col).min(line_end_byte);
    text.get(line_start_byte..want)
        .map(|s| s.chars().count())
        .unwrap_or(0)
}

/// Is highlighting available for this buffer? `None` for unnamed buffers and
/// for files over the cap.
pub fn lang_for_buffer(buffer: &Buffer) -> Option<Lang> {
    let path = buffer.path.as_ref()?;
    if buffer.rope.len_bytes() > MAX_HIGHLIGHT_BYTES {
        return None;
    }
    Lang::for_path(path)
}
