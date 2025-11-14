package export_my_test_i

import (
	"fmt"
	. "wit_component/wit_types"
)

func ReadStream(x *StreamReader[Unit]) {
	defer x.Drop()

	assertEqual(len(x.Read(1)), 1)
	assert(!x.WriterDropped())

	assertEqual(len(x.Read(2)), 2)
	assert(!x.WriterDropped())
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
