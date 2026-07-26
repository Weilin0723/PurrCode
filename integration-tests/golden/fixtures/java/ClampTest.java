public final class ClampTest {
    public static void main(String[] args) {
        if (Clamp.clamp(7, 0, 5) != 5) {
            throw new AssertionError("upper bound was not applied");
        }
        if (Clamp.clamp(-2, 0, 5) != 0) {
            throw new AssertionError("lower bound was not applied");
        }
        if (Clamp.clamp(3, 0, 5) != 3) {
            throw new AssertionError("in-range value changed");
        }
    }
}
