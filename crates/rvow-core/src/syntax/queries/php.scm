; PHPUnit: a method named test..., a method with a #[Test] attribute, or a
; method after a /** @test */ docblock. The @name capture holds the method
; name.
(
  (method_declaration
    name: (name) @name) @test
  (#match? @name "^test")
)

(
  (method_declaration
    (attribute_list
      (attribute_group
        (attribute (name) @ann)))
    name: (name) @name) @test
  (#eq? @ann "Test")
)

(
  (_
    (comment) @doc
    .
    (method_declaration
      name: (name) @name) @test)
  (#match? @doc "@test\\b")
)
