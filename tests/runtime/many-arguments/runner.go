package main

import (
	test "wit_component/test_many_arguments_to_test"
)

//go:wasmexport wasi:cli/run@0.2.6#run
func run() int32 {
	test.ManyArguments(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16)
	return 0
}
