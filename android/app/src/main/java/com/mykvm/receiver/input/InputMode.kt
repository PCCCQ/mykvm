package com.mykvm.receiver.input

/**
 * How the receiver turns wire events into Android input.
 *
 * Neither mode is strictly better -- which one a user wants depends on the app
 * they are driving, so the choice is theirs (see the 鼠标模式 / 触摸模式 switch).
 */
enum class InputMode {

    /**
     * Injects `SOURCE_MOUSE` events, so Android's own mouse stack handles them.
     *
     * Gets hover, a genuine right-click and a genuine scroll wheel, and
     * mouse-aware apps (browsers, office, remote desktop, emulators) take their
     * native mouse paths. Left-click still reaches ordinary apps, because the
     * view framework delivers mouse `ACTION_DOWN`/`ACTION_UP` to `onTouchEvent`
     * regardless of source.
     */
    MOUSE,

    /**
     * Injects `SOURCE_TOUCHSCREEN` events.
     *
     * Works in every app including ones that ignore mice, but hover is
     * impossible, a right-click has to be faked as a long press, and a wheel
     * notch has to be faked as a short swipe.
     */
    TOUCH;

    val key: String get() = name.lowercase()

    companion object {
        /** Unknown or missing values fall back to [MOUSE]. */
        fun fromKey(value: String?): InputMode =
            entries.firstOrNull { it.key == value?.lowercase() } ?: MOUSE
    }
}