; Go: func TestXxx(t *testing.T). The @name capture holds the function name.
(
  (function_declaration
    name: (identifier) @name) @test
  (#match? @name "^Test")
)
