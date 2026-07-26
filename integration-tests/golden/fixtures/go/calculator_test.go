package calculator

import "testing"

func TestMultiply(t *testing.T) {
	if Multiply(3, 4) != 12 {
		t.Fatal("multiply returned the wrong result")
	}
}
