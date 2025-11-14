package main

import test "wit_component/a_b_the_test"

//go:wasmexport wasi:cli/run@0.2.6#run
func run() int32 {
	test.X()
	return 0
}
