; Jest, Vitest and mocha style: test('name', ...) and it('name', ...).
; A @test capture marks the unit as a test and a @name capture names it.
; The predicate shares the group with the pattern, so it filters that
; pattern's matches.
(
  (call_expression
    function: (identifier) @fn
    arguments: (arguments (string (string_fragment) @name))) @test
  (#eq? @fn "test")
)

(
  (call_expression
    function: (identifier) @fn
    arguments: (arguments (string (string_fragment) @name))) @test
  (#eq? @fn "it")
)

; A group: a container that splits into a header unit plus one unit per
; body item, so a test added inside it is its own unit. describe stays a
; test above, so a whole new group is free too.
(
  (call_expression
    function: (identifier) @fn
    arguments: (arguments (string (string_fragment)))) @group
  (#eq? @fn "describe")
)

; describe(...) also names a changed group like a test does.
(
  (call_expression
    function: (identifier) @fn
    arguments: (arguments (string (string_fragment) @name))) @test
  (#eq? @fn "describe")
)
