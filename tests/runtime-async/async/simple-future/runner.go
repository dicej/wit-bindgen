package main

import (
	test "wit_component/my_test_i"
	"wit_component/wit_async"
	"wit_component/wit_types"
)

//go:wasmimport [export]wasi:cli/run@0.3.0-rc-2025-09-16 [task-return]run
func taskReturn(result int32)

//go:wasmexport [async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func run() uint32 {
	return wit_async.Run(func() {
		write := make(chan bool)
		read := make(chan wit_types.Unit)

		{
			tx, rx := test.MakeFutureUnit()
			go func() {
				write <- tx.Write(wit_types.Unit{})
			}()
			go func() {
				test.ReadFuture(rx)
				read <- wit_types.Unit{}
			}()
			(<-read)
			assert(<-write)
		}

		{
			tx, rx := test.MakeFutureUnit()
			go func() {
				write <- tx.Write(wit_types.Unit{})
			}()
			go func() {
				test.DropFuture(rx)
				read <- wit_types.Unit{}
			}()
			(<-read)
			assert(!(<-write))
		}

		taskReturn(0)
	})
}

//go:wasmexport [callback][async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func callback(event0 uint32, event1 uint32, event2 uint32) uint32 {
	return wit_async.Callback(event0, event1, event2)
}

func assert(v bool) {
	if !v {
		panic("assertion failed")
	}
}
