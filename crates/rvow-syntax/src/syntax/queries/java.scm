; JUnit: a method annotated @Test, @ParameterizedTest, @RepeatedTest or
; @TestFactory. The annotations are children of the method's modifiers. The
; @name capture holds the method name.
(
  (method_declaration
    (modifiers
      [(marker_annotation name: (identifier) @ann)
       (annotation name: (identifier) @ann)])
    name: (identifier) @name) @test
  (#match? @ann "^(Test|ParameterizedTest|RepeatedTest|TestFactory)$")
)
