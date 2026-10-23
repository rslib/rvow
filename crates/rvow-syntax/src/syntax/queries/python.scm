; pytest: def test_..., and unittest methods, which are also test_*.
; The @name capture holds the function name.
(
  (function_definition
    name: (identifier) @name) @test
  (#match? @name "^test_")
)
