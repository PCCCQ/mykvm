package com.mykvm.receiver.input

import android.view.KeyEvent

/**
 * Windows virtual-key code -> Android key code.
 *
 * The desktop puts Windows VK codes on the wire (see `windows_vk_to_mac_key` in
 * the desktop's input.rs -- macOS translates from the same numbering), so this
 * is the Android half of that contract.
 *
 * Keys Android has no equivalent for map to null and are dropped rather than
 * guessed at, so a stray key can never turn into a surprising system action.
 */
object KeyMap {

    /** Returns the Android key code for [vk], or null when unsupported. */
    fun androidKeyCode(vk: Int): Int? = when (vk) {
        // Letters
        in 0x41..0x5A -> KeyEvent.KEYCODE_A + (vk - 0x41)

        // Digits (top row)
        in 0x30..0x39 -> KeyEvent.KEYCODE_0 + (vk - 0x30)

        // Function row
        in 0x70..0x7B -> KeyEvent.KEYCODE_F1 + (vk - 0x70)

        // Modifiers
        0x10, 0xA0 -> KeyEvent.KEYCODE_SHIFT_LEFT
        0xA1 -> KeyEvent.KEYCODE_SHIFT_RIGHT
        0x11, 0xA2 -> KeyEvent.KEYCODE_CTRL_LEFT
        0xA3 -> KeyEvent.KEYCODE_CTRL_RIGHT
        0x12, 0xA4 -> KeyEvent.KEYCODE_ALT_LEFT
        0xA5 -> KeyEvent.KEYCODE_ALT_RIGHT
        0x5B -> KeyEvent.KEYCODE_META_LEFT
        0x5C -> KeyEvent.KEYCODE_META_RIGHT

        // Whitespace / editing
        0x08 -> KeyEvent.KEYCODE_DEL
        0x09 -> KeyEvent.KEYCODE_TAB
        0x0D -> KeyEvent.KEYCODE_ENTER
        0x1B -> KeyEvent.KEYCODE_ESCAPE
        0x20 -> KeyEvent.KEYCODE_SPACE

        // Navigation
        0x21 -> KeyEvent.KEYCODE_PAGE_UP
        0x22 -> KeyEvent.KEYCODE_PAGE_DOWN
        0x23 -> KeyEvent.KEYCODE_MOVE_END
        0x24 -> KeyEvent.KEYCODE_MOVE_HOME
        0x25 -> KeyEvent.KEYCODE_DPAD_LEFT
        0x26 -> KeyEvent.KEYCODE_DPAD_UP
        0x27 -> KeyEvent.KEYCODE_DPAD_RIGHT
        0x28 -> KeyEvent.KEYCODE_DPAD_DOWN
        0x2D -> KeyEvent.KEYCODE_INSERT
        0x2E -> KeyEvent.KEYCODE_FORWARD_DEL
        0x2C -> KeyEvent.KEYCODE_SYSRQ
        0x13 -> KeyEvent.KEYCODE_BREAK

        // Locks
        0x14 -> KeyEvent.KEYCODE_CAPS_LOCK
        0x90 -> KeyEvent.KEYCODE_NUM_LOCK
        0x91 -> KeyEvent.KEYCODE_SCROLL_LOCK

        // OEM punctuation (US layout)
        0xBA -> KeyEvent.KEYCODE_SEMICOLON
        0xBB -> KeyEvent.KEYCODE_EQUALS
        0xBC -> KeyEvent.KEYCODE_COMMA
        0xBD -> KeyEvent.KEYCODE_MINUS
        0xBE -> KeyEvent.KEYCODE_PERIOD
        0xBF -> KeyEvent.KEYCODE_SLASH
        0xC0 -> KeyEvent.KEYCODE_GRAVE
        0xDB -> KeyEvent.KEYCODE_LEFT_BRACKET
        0xDC -> KeyEvent.KEYCODE_BACKSLASH
        0xDD -> KeyEvent.KEYCODE_RIGHT_BRACKET
        0xDE -> KeyEvent.KEYCODE_APOSTROPHE

        // Numpad
        in 0x60..0x69 -> KeyEvent.KEYCODE_NUMPAD_0 + (vk - 0x60)
        0x6A -> KeyEvent.KEYCODE_NUMPAD_MULTIPLY
        0x6B -> KeyEvent.KEYCODE_NUMPAD_ADD
        0x6C -> KeyEvent.KEYCODE_NUMPAD_COMMA
        0x6D -> KeyEvent.KEYCODE_NUMPAD_SUBTRACT
        0x6E -> KeyEvent.KEYCODE_NUMPAD_DOT
        0x6F -> KeyEvent.KEYCODE_NUMPAD_DIVIDE

        // Media / browser / volume.
        // Android has no KEYCODE_BROWSER_HOME, so the browser-home key maps to
        // the system Home key -- which is what the key means to a user anyway.
        0xA6 -> KeyEvent.KEYCODE_BACK
        0xA7 -> KeyEvent.KEYCODE_FORWARD
        0xA8 -> KeyEvent.KEYCODE_REFRESH
        0xA9 -> KeyEvent.KEYCODE_MEDIA_STOP
        0xAA -> KeyEvent.KEYCODE_SEARCH
        0xAB -> KeyEvent.KEYCODE_BOOKMARK
        0xAC -> KeyEvent.KEYCODE_HOME
        0xAD -> KeyEvent.KEYCODE_VOLUME_MUTE
        0xAE -> KeyEvent.KEYCODE_VOLUME_DOWN
        0xAF -> KeyEvent.KEYCODE_VOLUME_UP
        0xB0 -> KeyEvent.KEYCODE_MEDIA_NEXT
        0xB1 -> KeyEvent.KEYCODE_MEDIA_PREVIOUS
        0xB2 -> KeyEvent.KEYCODE_MEDIA_STOP
        0xB3 -> KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE
        0x5D -> KeyEvent.KEYCODE_MENU

        else -> null
    }

    /** True for keys that contribute to `metaState` rather than acting alone. */
    fun isModifier(vk: Int): Boolean = when (vk) {
        0x10, 0xA0, 0xA1, 0x11, 0xA2, 0xA3, 0x12, 0xA4, 0xA5, 0x5B, 0x5C -> true
        else -> false
    }

    /**
     * `metaState` bit for a modifier VK, or 0 when [vk] is not a modifier.
     * Both left and right variants map to the same logical bit because Android
     * only tracks "shift is down", not which shift.
     */
    fun metaStateBit(vk: Int): Int = when (vk) {
        0x10, 0xA0, 0xA1 -> KeyEvent.META_SHIFT_ON
        0x11, 0xA2, 0xA3 -> KeyEvent.META_CTRL_ON
        0x12, 0xA4, 0xA5 -> KeyEvent.META_ALT_ON
        0x5B, 0x5C -> KeyEvent.META_META_ON
        else -> 0
    }
}