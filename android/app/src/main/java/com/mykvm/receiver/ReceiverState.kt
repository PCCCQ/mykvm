package com.mykvm.receiver

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/** Everything the UI needs to render, published by [KvmService]. */
data class ReceiverUiState(
    val running: Boolean = false,
    val shizukuRunning: Boolean = false,
    val injectionReady: Boolean = false,
    val overlayGranted: Boolean = false,
    val paired: Boolean = false,
    val pairingCode: String? = null,
    val pairingRequester: String? = null,
    val connectedController: String? = null,
    val deviceName: String = "",
    val peerId: String? = null,
    val quicPort: Int = 0,
    val discoveryPort: Int = 0,
    val lastError: String? = null,
)

/**
 * Process-wide UI state. The service writes, the activity collects; a
 * `StateFlow` keeps the last value so the activity renders correctly even when
 * it is created after the service has already started.
 */
object ReceiverState {

    private val _state = MutableStateFlow(ReceiverUiState())
    val state: StateFlow<ReceiverUiState> = _state.asStateFlow()

    val current: ReceiverUiState get() = _state.value

    fun update(block: (ReceiverUiState) -> ReceiverUiState) {
        _state.update(block)
    }

    fun reset() {
        _state.value = ReceiverUiState()
    }
}