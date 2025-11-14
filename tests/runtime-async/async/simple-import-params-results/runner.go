package main

import (
	"fmt"
	test "wit_component/a_b_i"
	"wit_component/wit_async"
)

//go:wasmimport [export]wasi:cli/run@0.3.0-rc-2025-09-16 [task-return]run
func taskReturn(result int32)

//go:wasmexport [async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func run() uint32 {
	return wit_async.Run(func() {
		test.OneArgument(1)
		assertEqual(test.OneResult(), 2)
		assertEqual(test.OneArgumentAndResult(3), 4)
		test.TwoArguments(5, 6)
		assertEqual(test.TwoArgumentsAndResult(7, 8), 9)
		taskReturn(0)
	})
}

//go:wasmexport [callback][async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func callback(event0 uint32, event1 uint32, event2 uint32) uint32 {
	return wit_async.Callback(event0, event1, event2)
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
