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
