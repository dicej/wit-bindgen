package main

import (
	"fmt"
	test "wit_component/cat"
)

//go:wasmexport wasi:cli/run@0.2.6#run
func run() int32 {
	test.Foo("hello")
	value := test.Bar()
	if value != "world" {
		panic(fmt.Sprintf("expected `world`; got `%v`", value))
	}
	return 0
}
