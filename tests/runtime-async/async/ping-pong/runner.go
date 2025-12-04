package main

import (
	"fmt"
	test "wit_component/my_test_i"
	"wit_component/wit_async"
	"wit_component/wit_types"
)

type Unit struct{}

//go:wasmimport [export]wasi:cli/run@0.3.0-rc-2025-09-16 [task-return]run
func taskReturn(result int32)

//go:wasmexport [async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func run() uint32 {
	return wit_async.Run(func() {
		{
			f1 := make(chan *wit_types.FutureReader[string])
			f2 := make(chan Unit)

			tx, rx := test.MakeFutureString()
			go func() {
				f1 <- test.Ping(rx, "world")
			}()

			go func() {
				tx.Write("hello")
				f2 <- Unit{}
			}()

			(<-f2)
			rx2 := (<-f1)
			assertEqual(rx2.Read(), "helloworld")
		}

		{
			f1 := make(chan Unit)
			f2 := make(chan Unit)

			tx, rx := test.MakeFutureString()
			go func() {
				assertEqual(test.Pong(rx), "helloworld")
				f1 <- Unit{}
			}()

			go func() {
				tx.Write("helloworld")
				f2 <- Unit{}
			}()

			(<-f2)
			(<-f1)
		}

		taskReturn(0)
	})
}

//go:wasmexport [callback][async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func callback(event0 uint32, event1 uint32, event2 uint32) uint32 {
	return wit_async.Callback(event0, event1, event2)
}

func assertEqual[T comparable](a, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
