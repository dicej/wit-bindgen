package wit_types

import (
	"runtime"
	"unsafe"
	"wit_component/wit_async"
	"wit_component/wit_runtime"
)

type FutureVtable[T any] struct {
	Size         uint32
	Align        uint32
	Read         func(handle int32, item unsafe.Pointer) uint32
	Write        func(handle int32, item unsafe.Pointer) uint32
	CancelRead   func(handle int32) uint32
	CancelWrite  func(handle int32) uint32
	DropReadable func(handle int32)
	DropWritable func(handle int32)
	Lift         func(src unsafe.Pointer) T
	Lower        func(pinner *runtime.Pinner, value T, dst unsafe.Pointer)
}

type FutureReader[T any] struct {
	vtable *FutureVtable[T]
	handle int32
}

func (f *FutureReader[T]) Read() T {
	if f.handle == 0 {
		panic("null future handle")
	}

	handle := f.handle
	f.handle = 0
	defer f.vtable.DropReadable(handle)

	pinner := runtime.Pinner{}
	defer pinner.Unpin()

	buffer := wit_runtime.Allocate(&pinner, uintptr(f.vtable.Size), uintptr(f.vtable.Align))

	code, _ := wit_async.FutureOrStreamWait(f.vtable.Read(handle, buffer), handle)

	switch code {
	case wit_async.RETURN_CODE_COMPLETED, wit_async.RETURN_CODE_DROPPED:
		if f.vtable.Lift == nil {
			return unsafe.Slice((*T)(buffer), 1)[0]
		} else {
			return f.vtable.Lift(buffer)
		}

	default:
		panic("todo: handle cancellation")
	}
}

func (f *FutureReader[T]) Drop() {
	handle := f.handle
	if handle != 0 {
		f.handle = 0
		f.vtable.DropReadable(handle)
	}
}

func (f *FutureReader[T]) TakeHandle() int32 {
	if f.handle == 0 {
		panic("null future handle")
	}

	handle := f.handle
	f.handle = 0
	return handle
}

func MakeFutureReader[T any](vtable *FutureVtable[T], handle int32) *FutureReader[T] {
	return &FutureReader[T]{vtable, handle}
}

type FutureWriter[T any] struct {
	vtable *FutureVtable[T]
	handle int32
}

func (f *FutureWriter[T]) Write(item T) bool {
	if f.handle == 0 {
		panic("null future handle")
	}

	handle := f.handle
	f.handle = 0
	defer f.vtable.DropWritable(handle)

	pinner := runtime.Pinner{}
	defer pinner.Unpin()

	var buffer unsafe.Pointer
	if f.vtable.Lower == nil {
		buffer = unsafe.Pointer(unsafe.SliceData([]T{item}))
		pinner.Pin(buffer)
	} else {
		buffer = wit_runtime.Allocate(&pinner, uintptr(f.vtable.Size), uintptr(f.vtable.Align))
		f.vtable.Lower(&pinner, item, buffer)
	}

	code, _ := wit_async.FutureOrStreamWait(f.vtable.Write(handle, buffer), handle)

	// TODO: restore handles to any unwritten resources, streams, or futures

	switch code {
	case wit_async.RETURN_CODE_COMPLETED:
		return true

	case wit_async.RETURN_CODE_DROPPED:
		return false

	default:
		panic("todo: handle cancellation")
	}
}

func (f *FutureWriter[T]) Drop() {
	handle := f.handle
	if handle != 0 {
		f.handle = 0
		f.vtable.DropWritable(handle)
	}
}

func MakeFutureWriter[T any](vtable *FutureVtable[T], handle int32) *FutureWriter[T] {
	return &FutureWriter[T]{vtable, handle}
}
