public final class Clamp {
    public static int clamp(int value, int minimum, int maximum) {
        return Math.max(maximum, Math.min(minimum, value));
    }
}
