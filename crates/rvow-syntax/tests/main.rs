//! The integration tests of the rvow-syntax crate, built as one binary. Each module is one former test file; the files live in suite/.

#[path = "suite/syntax_canonical.rs"]
mod syntax_canonical;
#[path = "suite/syntax_grammar.rs"]
mod syntax_grammar;
