; RSpec and Minitest spec: it, specify and example calls, and Minitest
; test "..." calls. The @name capture holds the description.
(
  (call
    method: (identifier) @fn
    arguments: (argument_list . (string (string_content) @name))) @test
  (#match? @fn "^(it|specify|example|test)$")
)

; A group: describe and context split into a header unit plus one unit per
; body item, so an example added inside it is its own unit.
(
  (call
    method: (identifier) @fn
    block: (do_block)) @group
  (#match? @fn "^(describe|context)$")
)

; describe and context also name a changed group like a test does.
(
  (call
    method: (identifier) @fn
    arguments: (argument_list . (string (string_content) @name))) @test
  (#match? @fn "^(describe|context)$")
)

(
  (call
    method: (identifier) @fn
    arguments: (argument_list . (constant) @name)) @test
  (#match? @fn "^(describe|context)$")
)
