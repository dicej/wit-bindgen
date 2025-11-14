package main

import (
	"fmt"
	test "wit_component/test_records_to_test"
	. "wit_component/wit_types"
)

//go:wasmexport wasi:cli/run@0.2.6#run
func run() int32 {
	a, b := test.MultipleResults()
	assertEqual(a, 4)
	assertEqual(b, 5)

	c, d := test.SwapTuple(Tuple2[uint8, uint32]{1, 2})
	assertEqual(c, 2)
	assertEqual(d, 1)

	return 0
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
