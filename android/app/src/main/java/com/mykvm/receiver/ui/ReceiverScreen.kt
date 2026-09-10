package com.mykvm.receiver.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mykvm.receiver.ReceiverState
import com.mykvm.receiver.ShizukuStatus
import com.mykvm.receiver.ReceiverUiState
import kotlinx.coroutines.delay

private val CardColor = Color(0xFF1A1D23)
private val OkColor = Color(0xFF3DD68C)
private val WarnColor = Color(0xFFF0B429)
private val BadColor = Color(0xFFF05252)
private val Muted = Color(0xFF9AA4B2)

@Composable
fun ReceiverScreen(
    onToggleService: (Boolean) -> Unit,
    onGrantShizuku: () -> Unit,
    onOpenShizuku: () -> Unit,
    onGrantOverlay: () -> Unit,
    onUnpair: () -> Unit,
) {
    val context = LocalContext.current
    val state by ReceiverState.state.collectAsState()

    // Shizuku can be started or stopped (and the permission granted or
    // revoked) entirely outside this app, so re-probe while the screen is up.
    LaunchedEffect(Unit) {
        while (true) {
            ShizukuStatus.refresh(context)
            delay(2000)
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Header(state)

        StatusCard(
            title = "接收服务",
            ok = state.running,
            detail = when {
                state.running -> "正在监听，等待电脑连接"
                else -> "已停止"
            },
            action = {
                Switch(checked = state.running, onCheckedChange = onToggleService)
            },
        )

        PrerequisiteCard(
            title = "Shizuku（注入键鼠）",
            ok = state.injectionReady,
            warning = !state.injectionReady && state.shizukuRunning,
            detail = when {
                state.injectionReady -> "已授权，可以注入鼠标和键盘"
                state.shizukuRunning -> "Shizuku 已运行，但本应用尚未获得授权"
                else -> "未检测到 Shizuku。无 root 时这是注入键盘的唯一方式"
            },
            primaryLabel = when {
                state.injectionReady -> null
                state.shizukuRunning -> "授权"
                else -> "打开 Shizuku"
            },
            onPrimary = if (state.shizukuRunning) onGrantShizuku else onOpenShizuku,
        )

        PrerequisiteCard(
            title = "悬浮窗权限",
            ok = state.overlayGranted,
            warning = false,
            detail = if (state.overlayGranted) {
                "已授权，可以显示光标"
            } else {
                "未授权，手机上看不到光标位置"
            },
            primaryLabel = if (state.overlayGranted) null else "授权",
            onPrimary = onGrantOverlay,
        )

        PairingCard(state, onUnpair)

        InfoCard(state)

        state.lastError?.let { error ->
            Text(error, color = BadColor, fontSize = 13.sp)
        }
    }
}

@Composable
private fun Header(state: ReceiverUiState) {
    Column {
        Text(
            "MyKVM 被控端",
            fontSize = 26.sp,
            fontWeight = FontWeight.SemiBold,
            color = Color.White,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            if (state.paired) "已配对 · ${state.deviceName}" else "未配对 · ${state.deviceName}",
            color = Muted,
            fontSize = 14.sp,
        )
    }
}

@Composable
private fun StatusCard(
    title: String,
    ok: Boolean,
    detail: String,
    action: @Composable () -> Unit,
) {
    Card(
        colors = CardDefaults.cardColors(containerColor = CardColor),
        shape = RoundedCornerShape(14.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Dot(if (ok) OkColor else Muted)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(title, color = Color.White, fontSize = 16.sp, fontWeight = FontWeight.Medium)
                Text(detail, color = Muted, fontSize = 13.sp)
            }
            action()
        }
    }
}

@Composable
private fun PrerequisiteCard(
    title: String,
    ok: Boolean,
    warning: Boolean,
    detail: String,
    primaryLabel: String?,
    onPrimary: () -> Unit,
) {
    val color = when {
        ok -> OkColor
        warning -> WarnColor
        else -> BadColor
    }

    Card(
        colors = CardDefaults.cardColors(containerColor = CardColor),
        shape = RoundedCornerShape(14.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Dot(color)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(title, color = Color.White, fontSize = 16.sp, fontWeight = FontWeight.Medium)
                Text(detail, color = Muted, fontSize = 13.sp)
            }
            if (primaryLabel != null) {
                Spacer(Modifier.width(12.dp))
                Button(onClick = onPrimary) { Text(primaryLabel) }
            }
        }
    }
}

@Composable
private fun PairingCard(state: ReceiverUiState, onUnpair: () -> Unit) {
    Card(
        colors = CardDefaults.cardColors(containerColor = CardColor),
        shape = RoundedCornerShape(14.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp)) {
            Text("配对", color = Color.White, fontSize = 16.sp, fontWeight = FontWeight.Medium)
            Spacer(Modifier.height(8.dp))

            when {
                state.pairingCode != null -> {
                    Text(
                        "在电脑端输入这个验证码：",
                        color = Muted,
                        fontSize = 13.sp,
                    )
                    Spacer(Modifier.height(6.dp))
                    Text(
                        state.pairingCode,
                        color = OkColor,
                        fontSize = 44.sp,
                        fontWeight = FontWeight.Bold,
                        fontFamily = FontFamily.Monospace,
                    )
                    state.pairingRequester?.let {
                        Text("来自 $it", color = Muted, fontSize = 13.sp)
                    }
                }

                state.paired -> {
                    Text(
                        "已与「${state.connectedController ?: "电脑"}」配对",
                        color = OkColor,
                        fontSize = 14.sp,
                    )
                    Spacer(Modifier.height(10.dp))
                    OutlinedButton(onClick = onUnpair) { Text("解除配对") }
                }

                else -> {
                    Text(
                        "在电脑端打开 MyKVM，在「设备」里添加本机并点击配对，验证码会显示在这里。",
                        color = Muted,
                        fontSize = 13.sp,
                    )
                }
            }
        }
    }
}

@Composable
private fun InfoCard(state: ReceiverUiState) {
    Card(
        colors = CardDefaults.cardColors(containerColor = CardColor),
        shape = RoundedCornerShape(14.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text("连接信息", color = Color.White, fontSize = 16.sp, fontWeight = FontWeight.Medium)
            InfoRow("设备名", state.deviceName)
            InfoRow("设备 ID", state.peerId ?: "-")
            InfoRow("QUIC 端口", state.quicPort.takeIf { it > 0 }?.toString() ?: "-")
            InfoRow("发现端口", state.discoveryPort.takeIf { it > 0 }?.toString() ?: "-")
            Spacer(Modifier.height(2.dp))
            Text(
                "有线：插上 USB 后开启「USB 网络共享」。无线：手机与电脑接同一个 Wi-Fi。",
                color = Muted,
                fontSize = 12.sp,
            )
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth()) {
        Text(label, color = Muted, fontSize = 13.sp, modifier = Modifier.width(84.dp))
        Text(value, color = Color.White, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
    }
}

@Composable
private fun Dot(color: Color) {
    Box(
        Modifier
            .width(10.dp)
            .height(10.dp)
            .background(color, RoundedCornerShape(50)),
    )
}
