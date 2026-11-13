// stub.rs: the `syntax` feature turned off.
//
// Same public surface as the real module, with no grammar, no parser and no
// query: every file is simply "no language", so nothing is highlighted and no
// parsing ever happens. This file exists so `--no-default-features` builds and
// the rest of the editor needs no `#[cfg]` at its call sites.

// A stub is dead by definition: no language can ever be produced without
// grammars, which is the whole point of turning the feature off.
#![allow(dead_code)]

use crate::editor::buffer::Buffer;

pub const MAX_HIGHLIGHT_BYTES: usize = 0;
pub const BIG_PARSE_BYTES: usize = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {}

/// The same variants as the real enum, because the theme maps all of them. With
/// no language there is no span, so none of them is ever produced.
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
    Heading,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start_col: u32,
    pub end_col: u32,
    pub kind: TokenKind,
}

/// A placeholder with the methods `Editor` calls. Every one of them is a no-op,
/// so the editor behaves exactly as it did before highlighting existed.
pub struct Highlighter {
    /// Never `Some`: no tree exists without grammars.
    tree: Option<()>,
}

impl Highlighter {
    pub fn new(_lang: Lang) -> Option<Self> {
        None
    }

    pub fn edit(&mut self, _e: ()) {}

    pub fn reparse(&mut self, _buffer: &Buffer, _version: u64) -> bool {
        false
    }

    pub fn spans_for_lines(
        &mut self,
        _buffer: &Buffer,
        first: usize,
        last: usize,
        _version: u64,
    ) -> Vec<Vec<Span>> {
        vec![Vec::new(); last.saturating_sub(first)]
    }

    pub fn wants_async(&self, _buffer: &Buffer) -> bool {
        false
    }

    /// Kept so the field type matches; nothing ever reads it.
    pub fn tree(&self) -> Option<()> {
        self.tree
    }
}

pub fn lang_for_buffer(_buffer: &Buffer) -> Option<Lang> {
    None
}
