; xUnit, NUnit and MSTest: a method with [Fact], [Theory], [Test],
; [TestCase(...)] or [TestMethod]. The @name capture holds the method name.
(
  (method_declaration
    (attribute_list
      (attribute name: (identifier) @ann))
    name: (identifier) @name) @test
  (#match? @ann "^(Fact|Theory|Test|TestCase|TestMethod)$")
)
