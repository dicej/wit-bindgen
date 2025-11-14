package export_my_test_i

import (
	"slices"
	. "wit_component/wit_types"
)

func ReadStream(x *StreamReader[uint8]) {
	defer x.Drop()

	assert(slices.Equal(x.Read(1), []uint8{0}))
	assert(!x.WriterDropped())

	assert(slices.Equal(x.Read(2), []uint8{1, 2}))
	assert(!x.WriterDropped())

	assert(slices.Equal(x.Read(1), []uint8{3}))
	assert(!x.WriterDropped())

	assert(slices.Equal(x.Read(1), []uint8{4}))
	assert(!x.WriterDropped())
}

func assert(v bool) {
	if !v {
		panic("assertion failed")
	}
}
