package main

import (
	"fmt"
	test "wit_component/test_strings_to_test"
)

//go:wasmexport wasi:cli/run@0.2.6#run
func run() int32 {
	test.TakeBasic("latin utf16")
	assertEqual(test.ReturnUnicode(), "🚀🚀🚀 𠈄𓀀")
	assertEqual(test.ReturnEmpty(), "")
	assertEqual(test.Roundtrip("🚀🚀🚀 𠈄𓀀"), "🚀🚀🚀 𠈄𓀀")
	return 0
}

func assertEqual(a string, b string) {
	if a != b {
		panic(fmt.Sprintf("`%v` not equal to `%v`", a, b))
	}
}
