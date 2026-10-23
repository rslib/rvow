; Unity: a `test_` function in a file that includes unity.h, and the
; RUN_TEST(test_name) line of main. A function keeps its name, setUp and
; tearDown are no tests, and a file with no Unity include marks nothing. The
; pattern roots at the file, so a function nested in a preprocessor block is
; no test.
(
  (translation_unit
    (preproc_include path: (_) @include)
    (function_definition
      declarator: (function_declarator declarator: (identifier) @name)) @test)
  (#match? @include "unity")
  (#match? @name "^test_")
)

(
  (call_expression
    function: (identifier) @fn
    arguments: (argument_list . (identifier) @name .)) @test
  (#eq? @fn "RUN_TEST")
  (#match? @name "^test_")
)

; main of a file that holds RUN_TEST is a group: its header is one unit and
; each statement is its own unit, so a new RUN_TEST line is a unit of its
; own. Any other main stays one unit.
(
  (function_definition
    declarator: (function_declarator declarator: (identifier) @fn)
    body: (compound_statement
      (expression_statement
        (call_expression function: (identifier) @run)))) @group
  (#eq? @fn "main")
  (#eq? @run "RUN_TEST")
)
