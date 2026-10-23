; GoogleTest macros: TEST(Suite, Name), TEST_F(Fixture, Name) and
; TEST_P(Parameterized, Name). tree-sitter-cpp parses the macro as a
; function_definition whose declarator names the macro. Two @name captures
; hold the suite and the test; the item names the test `Suite.Name`.
(
  (function_definition
    declarator: (function_declarator
      declarator: (identifier) @fn
      parameters: (parameter_list
        (parameter_declaration type: (type_identifier) @name)
        .
        (parameter_declaration type: (type_identifier) @name)))) @test
  (#eq? @fn "TEST")
)

(
  (function_definition
    declarator: (function_declarator
      declarator: (identifier) @fn
      parameters: (parameter_list
        (parameter_declaration type: (type_identifier) @name)
        .
        (parameter_declaration type: (type_identifier) @name)))) @test
  (#eq? @fn "TEST_F")
)

(
  (function_definition
    declarator: (function_declarator
      declarator: (identifier) @fn
      parameters: (parameter_list
        (parameter_declaration type: (type_identifier) @name)
        .
        (parameter_declaration type: (type_identifier) @name)))) @test
  (#eq? @fn "TEST_P")
)

; doctest and Catch2: TEST_CASE("adds") { ... }, TEST_SUITE("calc") { ... },
; SCENARIO("s") { ... } and TEST_CASE_TEMPLATE("t", T, int) { ... }.
; tree-sitter-cpp parses a macro with no `;` as two siblings: an
; expression_statement with a missing `;` and a compound_statement. The
; pattern matches the pair under its parent, and @test spans the header
; statement. The name is the first string, also inside doctest decorators
; such as `"adds" * doctest::skip()`.
(
  (_
    (expression_statement
      (call_expression
        function: (identifier) @fn
        arguments: (argument_list
          .
          [
            (string_literal (string_content) @name)
            (binary_expression left: (string_literal (string_content) @name))
            (binary_expression left: (binary_expression left: (string_literal (string_content) @name)))
            (binary_expression left: (binary_expression left: (binary_expression left: (string_literal (string_content) @name))))
          ]))) @test
    .
    (compound_statement))
  (#match? @fn "^(DOCTEST_)?(TEST_CASE|TEST_CASE_TEMPLATE|SCENARIO|TEST_SUITE)$|^TEMPLATE_TEST_CASE$")
)

; TEST_CASE_FIXTURE(Calc, "adds") and Catch2 TEST_CASE_METHOD(Calc, "adds"):
; the name is the string after the fixture.
(
  (_
    (expression_statement
      (call_expression
        function: (identifier) @fn
        arguments: (argument_list
          (identifier)
          .
          (string_literal (string_content) @name)))) @test
    .
    (compound_statement))
  (#match? @fn "^(DOCTEST_)?TEST_CASE_(FIXTURE|METHOD)$")
)

; TEST_SUITE is a group: a header unit plus one unit per body item, so a
; test case inside it is its own unit. SUBCASE and SECTION stay inside
; their test case, which is never a group.
(
  (expression_statement
    (call_expression
      function: (identifier) @fn
      arguments: (argument_list . (string_literal)))) @group
  (#match? @fn "^(DOCTEST_)?TEST_SUITE$")
)
