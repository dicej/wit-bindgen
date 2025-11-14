package wit_types

import (
	"runtime"
	"unsafe"
	"wit_component/wit_async"
	"wit_component/wit_runtime"
)

type StreamVtable[T any] struct {
	Size         uint32
	Align        uint32
	Read         func(handle int32, items unsafe.Pointer, length uint32) uint32
	Write        func(handle int32, items unsafe.Pointer, length uint32) uint32
	CancelRead   func(handle int32) uint32
	CancelWrite  func(handle int32) uint32
	DropReadable func(handle int32)
	DropWritable func(handle int32)
	Lift         func(src unsafe.Pointer) T
	Lower        func(pinner *runtime.Pinner, value T, dst unsafe.Pointer)
}

type StreamReader[T any] struct {
	vtable        *StreamVtable[T]
	handle        int32
	writerDropped bool
}

func (s *StreamReader[T]) WriterDropped() bool {
	return s.writerDropped
}

func (s *StreamReader[T]) Read(maxCount uint32) []T {
	if s.handle == 0 {
		panic("null stream handle")
	}

	if s.writerDropped {
		return []T{}
	}

	pinner := runtime.Pinner{}
	defer pinner.Unpin()

	buffer := wit_runtime.Allocate(&pinner, uintptr(s.vtable.Size*maxCount), uintptr(s.vtable.Align))

	code, count := wit_async.FutureOrStreamWait(s.vtable.Read(s.handle, buffer, maxCount), s.handle)

	if code == wit_async.RETURN_CODE_DROPPED {
		s.writerDropped = true
	}

	if s.vtable.Lift == nil {
		return unsafe.Slice((*T)(buffer), count)
	} else {
		result := make([]T, 0, count)
		for i := 0; i < int(count); i++ {
			result = append(result, s.vtable.Lift(unsafe.Add(buffer, i*int(s.vtable.Size))))
		}
		return result
	}
}

func (s *StreamReader[T]) ReadInto(dst []T) uint32 {
	if s.handle == 0 {
		panic("null stream handle")
	}

	if s.vtable.Lift != nil {
		panic("cannot use `ReadInto` with this payload type")
	}

	if s.writerDropped {
		return 0
	}

	pinner := runtime.Pinner{}
	defer pinner.Unpin()

	buffer := unsafe.Pointer(unsafe.SliceData(dst))
	pinner.Pin(buffer)

	code, count := wit_async.FutureOrStreamWait(s.vtable.Read(s.handle, buffer, uint32(len(dst))), s.handle)

	if code == wit_async.RETURN_CODE_DROPPED {
		s.writerDropped = true
	}

	return count
}

func (s *StreamReader[T]) Drop() {
	handle := s.handle
	if handle != 0 {
		s.handle = 0
		s.vtable.DropReadable(handle)
	}
}

func (s *StreamReader[T]) TakeHandle() int32 {
	if s.handle == 0 {
		panic("null stream handle")
	}

	handle := s.handle
	s.handle = 0
	return handle
}

func MakeStreamReader[T any](vtable *StreamVtable[T], handle int32) *StreamReader[T] {
	return &StreamReader[T]{vtable, handle, false}
}

type StreamWriter[T any] struct {
	vtable        *StreamVtable[T]
	handle        int32
	readerDropped bool
}

func (s *StreamWriter[T]) ReaderDropped() bool {
	return s.readerDropped
}

func (s *StreamWriter[T]) Write(items []T) uint32 {
	if s.handle == 0 {
		panic("null stream handle")
	}

	if s.readerDropped {
		return 0
	}

	pinner := runtime.Pinner{}
	defer pinner.Unpin()

	writeCount := uint32(len(items))

	var buffer unsafe.Pointer
	if s.vtable.Lower == nil {
		buffer = unsafe.Pointer(unsafe.SliceData(items))
		pinner.Pin(buffer)
	} else {
		buffer = wit_runtime.Allocate(&pinner, uintptr(s.vtable.Size*writeCount), uintptr(s.vtable.Align))
		for index, item := range items {
			s.vtable.Lower(&pinner, item, unsafe.Add(buffer, index*int(s.vtable.Size)))
		}
	}

	code, count := wit_async.FutureOrStreamWait(s.vtable.Write(s.handle, buffer, writeCount), s.handle)

	// TODO: restore handles to any unwritten resources, streams, or futures

	if code == wit_async.RETURN_CODE_DROPPED {
		s.readerDropped = true
	}

	return count
}

func (s *StreamWriter[T]) WriteAll(items []T) uint32 {
	offset := uint32(0)
	count := uint32(len(items))
	for offset < count && !s.readerDropped {
		offset += s.Write(items[offset:])
	}
	return offset
}

func (s *StreamWriter[T]) Drop() {
	handle := s.handle
	if handle != 0 {
		s.handle = 0
		s.vtable.DropWritable(handle)
	}
}

func MakeStreamWriter[T any](vtable *StreamVtable[T], handle int32) *StreamWriter[T] {
	return &StreamWriter[T]{vtable, handle, false}
}
