package main

import (
	"fmt"
	test "wit_component/my_test_i"
	"wit_component/wit_async"
)

type Unit struct{}

//go:wasmimport [export]wasi:cli/run@0.3.0-rc-2025-09-16 [task-return]run
func taskReturn(result int32)

//go:wasmexport [async-lift]wasi:cli/run@0.3.0-rc-2025-09-16#run
func run() uint32 {
	return wit_async.Run(func() {
		write := make(chan Unit)
		read := make(chan Unit)

		tx, rx := test.MakeStreamU8()
		go func() {
			assertEqual(tx.Write([]uint8{0}), 1)
			assert(!tx.ReaderDropped())

			assertEqual(tx.Write([]uint8{1, 2}), 2)
			assert(!tx.ReaderDropped())

			assertEqual(tx.Write([]uint8{3, 4}), 2)

			assertEqual(tx.Write([]uint8{0}), 0)
			assert(tx.ReaderDropped())

			write <- Unit{}
		}()

		go func() {
			test.ReadStream(rx)
			read <- Unit{}
		}()

		(<-read)
		(<-write)

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

func assert(v bool) {
	if !v {
		panic("assertion failed")
	}
}
