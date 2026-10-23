; A #[test] or #[tokio::test] function. The attribute is a sibling of the
; function in tree-sitter-rust, so the pattern's root is a wildcard: it
; matches the parent (a file or a module body) and the anchor keeps the
; attribute right before the function. The @name capture holds the
; function name.
(
  (_
    (attribute_item) @attr
    .
    (function_item
      name: (identifier) @name) @test)
  (#match? @attr "^#\\[(test\\]|tokio::test)")
)
