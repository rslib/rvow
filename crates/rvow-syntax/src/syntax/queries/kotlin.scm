; JUnit and kotlin.test: a function annotated @Test, @ParameterizedTest,
; @RepeatedTest or @TestFactory. The @name capture holds the function name.
(
  (function_declaration
    (modifiers
      (annotation
        [(user_type (identifier) @ann)
         (constructor_invocation (user_type (identifier) @ann))]))
    (identifier) @name) @test
  (#match? @ann "^(Test|ParameterizedTest|RepeatedTest|TestFactory)$")
)
