package main

import (
	test "wit_component/a_b_i"
	"wit_component/wit_async"
)

//go:wasmimport [export]wasi:cli/run@0.3.0-rc-2025-09-16 [task-return]run
func taskReturn(result int32)

//go:wasmexport [async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func run() uint32 {
	return wit_async.Run(func() {
		test.F()
		taskReturn(0)
	})
}

//go:wasmexport [callback][async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func callback(event0 uint32, event1 uint32, event2 uint32) uint32 {
	return wit_async.Callback(event0, event1, event2)
}
