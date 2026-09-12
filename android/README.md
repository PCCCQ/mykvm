# MyKVM 被控端（Android）

把 Android 手机/平板变成 MyKVM 的**被控端**：用电脑的鼠标和键盘操作手机。
Android 端只接收，不控制别人。

---

## 一、使用前提

| 项目 | 要求 |
|---|---|
| 系统 | Android 11 (API 30) 及以上 |
| **Shizuku** | **必需**。无 root 时，这是唯一能注入键盘的途径，见下文 |
| 悬浮窗权限 | 用于显示光标位置（`SYSTEM_ALERT_WINDOW`） |
| 通知权限 | 前台服务常驻通知（API 33+） |

### 为什么必须装 Shizuku

Android 的公开 API 里，`AccessibilityService.dispatchGesture()` 只能合成**触摸**，
**没有任何接口可以注入按键**。想发 `KeyEvent` 必须调用
`InputManager.injectInputEvent`，而它要求 `INJECT_EVENTS` 权限 —— 该权限只授予
system / root / **shell(uid 2000)**。

Shizuku 的作用就是把 shell 权限借给普通应用，scrcpy 走的也是同一原理。

- **首次安装**：装 [Shizuku](https://shizuku.rikka.app/)，在开发者选项里打开
  「无线调试」，然后在 Shizuku 应用内启动（Android 11+ **全程不需要电脑**），
  回到本应用点「授权」。
- **重启后**：无 root 的固有限制，Shizuku 需要重新启动一次（应用首页会提示）。

---

## 二、连接方式

两种方式的传输层完全相同，都是**局域网 UDP**，不需要额外配置。

### 有线（推荐，延迟最低）

1. USB 连接手机和电脑
2. 手机：设置 → 网络 → **USB 网络共享**，打开
3. 电脑会多出一张网卡（通常是 `192.168.42.x`），MyKVM 桌面端会自动发现手机

> 传输层是 IP，所以「有线」实际是「USB 上的 IP」。这样桌面端零改动就能复用
> 现成的发现逻辑（它本来就遍历本机所有网卡做子网广播）。
> 有线模式下建议关掉手机 Wi-Fi，避免 Android 把广播发到默认网络。

### 无线

手机与电脑连同一个 Wi-Fi 即可。

---

## 三、配对

和桌面端原有流程一致：

1. 桌面端打开 MyKVM → 「设备」→ 添加本机 IP → 点配对
2. 手机端弹出 6 位验证码
3. 把验证码填进桌面端

配对完成后，手机屏幕上会出现一个光标，鼠标推过屏幕边缘即可落到手机上。

---

## 四、输入模式：鼠标 / 触摸

App 首页有一个开关，桌面端不需要任何改动。

### 鼠标模式（默认）

以 `SOURCE_MOUSE` 注入，走 Android 自己的鼠标栈。

| 电脑操作 | Android 行为 |
|---|---|
| 移动鼠标 | `ACTION_HOVER_MOVE` —— 支持悬停高亮 |
| 左键 | `BUTTON_PRIMARY` 的 `ACTION_DOWN`/`ACTION_UP` |
| 右键 | `BUTTON_SECONDARY` —— **真正的右键菜单** |
| 中键 | `BUTTON_TERTIARY` |
| 鼠标侧键 | `BUTTON_BACK` / `BUTTON_FORWARD` |
| 滚轮 | `ACTION_SCROLL` + `AXIS_VSCROLL`/`AXIS_HSCROLL` —— **真正的滚轮** |
| 键盘 | `KeyEvent` + `SOURCE_KEYBOARD`，修饰键（Ctrl/Alt/Shift/Win）有效 |

左键在所有 App 里都能用：View 框架会把鼠标的 `ACTION_DOWN`/`ACTION_UP`
照样送进 `onTouchEvent`，与事件源无关。右键和滚轮只有支持鼠标的 App 才响应。

### 触摸模式

以 `SOURCE_TOUCHSCREEN` 注入合成触摸。兼容所有 App（包括完全不认鼠标的游戏），
但能力有上限：

| 电脑操作 | Android 行为 |
|---|---|
| 移动鼠标 | 只移动自绘光标，**不注入**（触摸设备没有悬停） |
| 左键 | 合成 tap / 拖拽 |
| 右键 | 伪造成**长按** |
| 中键 / 侧键 | 返回键 |
| 滚轮 | 伪造成**短距离滑动** |

> 遇到某个 App 在鼠标模式下右键无反应，切到触摸模式即可 —— 这就是这个开关存在的意义。

### 关于自绘光标

**两种模式都使用应用自绘的悬浮光标**，因为 Android 不会为注入的鼠标事件画光标
（详见第五节）。所以悬浮窗权限是必需的。

屏幕以**物理像素 + scale 1.0** 上报，所以桌面端传来的坐标就是注入坐标，没有换算。

---

## 五、实现要点（真机踩坑记录）

以下三点都是**只在真机上才会暴露**的问题，改动它们前请先读完。

### 1. `injectInputEvent` 的 AIDL 事务号会随 Android 版本变化

`InputManager.injectInputEvent` 是隐藏接口 `IInputManager` 的方法。**不能用固定的
事务号调用**——方法在接口里的位置每个版本都在变，而且运行时读不到：

- 生成的 `TRANSACTION_*` 常量是 `static final int`，被 javac 内联后从
  `framework.jar` 里剥离了，反射会得到
  `NoSuchFieldException: TRANSACTION_injectInputEvent`。
- 调错事务号**不会报错**：服务返回空 reply，`readException()` 读不出异常，
  事件被静默丢弃。

实测自 AOSP `core/java/android/hardware/input/IInputManager.aidl`：

| API | Android | 事务号 |
| --- | --- | --- |
| 28–32 | 9 – 12L | 8 |
| 33 | 13 | 9 |
| 34 | 14 | 10 |
| 35 | 15 | 11 |

表在 `input/ShizukuInjector.kt` 的 `transactionCode()` 里。**升级 Android 大版本后
如果注入失效，第一个要查的就是这里。**

另外两点同样重要：

- AIDL 对 `in` 的 Parcelable 参数会**先写一个 presence int**（`writeInt(1)`）
  再写 parcel，漏掉它服务会解析错位。
- `injectInputEvent` 返回 `boolean`，必须读出来判断。不读的话，「调错事务号」
  和「调用成功」在日志上完全一样——这是排查时最大的坑。

### 2. 被控端不做单播扫描

桌面端发现不到设备时会扫整个 /24 网段（2032 个目标）。**被控端绝不能照做**：
接收端是**被动应答者**，桌面端本来就会扫。

最初的实现照搬了桌面端的扫描逻辑，每 15 秒往 254 个不存在的地址 × 8 个端口发包，
每个包都触发一次 ARP 解析，把发现循环阻塞到连桌面的探测包都来不及回——
现象是「桌面端完全发现不了平板」。

`discovery.rs` 现在只做两件事：每 3 秒广播一次自述，以及**立即应答**收到的包。

### 3. 悬浮光标的所有窗口操作必须在主线程

`WindowManager.addView()` 会构造 `ViewRootImpl`，它在**调用线程**上装 `Handler`。
从轮询线程调用会抛：

```
Can't create handler inside thread Thread[mykvm-poll] that has not called Looper.prepare()
```

`VirtualCursor` 因此把 `show` / `hide` / `moveTo` 全部投递到主线程，并把同一帧内的
多次移动合并成一次 relayout。

---

### 4. 不要指望系统为注入事件画光标

Android 的鼠标光标由 `PointerController` 驱动，而它**只跟随真实输入设备**，
不跟随 `injectInputEvent`。实机表现：

- 注入鼠标事件时，`dumpsys input` 里始终是 `PointerController: Presentation: SPOT`
  （触摸点样式），从未切成鼠标箭头样式。
- 把自绘光标关掉、完整扫一遍屏幕，截图里什么都没留下。

所以悬浮光标在两种模式下都必须保留 —— 触摸模式是因为触摸设备本就没有光标，
鼠标模式是因为系统不会替我们画。

---

### 5. 屏幕尺寸变化不再重启接收端

旋转屏幕、进出电脑模式都会触发 `onConfigurationChanged`。**不要**在这里
`stopReceiver(); startReceiver()`：那会关掉 QUIC endpoint，电脑端正在用的连接被
掐断，日志里表现为连续多条

```
QUIC send to <ip>:47834 failed: ... the server refused to accept a new connection
```

现在的做法是把屏幕尺寸放进 `ReceiverConfig` 里共享的 `Arc<Mutex<Screen>>`，由
Kotlin 调 `NativeCore.nativeSetScreenSize(w, h)` 原地更新；discovery 线程每 3 秒的
announce 会自己带上新尺寸，电脑端无需重连。`Receiver::stop()` 也做了幂等，
避免 `nativeStop` + `Drop` 打印两条 `receiver stopped`。

### 6. 已配对的接收端仍然接受重新配对

配对链路的授权凭据是**屏幕上显示的验证码**，所以接收端只要是 client 角色就接受
任何 server 的 `pair-request`。早期版本要求"未配对或请求者已被认识"才发验证码，
导致存下来的 controller 记录一旦过期（电脑端重装后证书轮换、或 IP 在 Wi-Fi/USB
共享之间变化），电脑端点配对只会收到 “no pairing challenge received”，用户只能两边
清空配对才能恢复。

同时 discovery 线程会像桌面端的 `refresh_paired_controller_keys` 一样，用
`can_refresh_controller_identity` **刷新**已存的 controller 记录（id / 证书公钥 /
host / ip / 名称）。注意这个判定比"能不能重新配对"更严格：只按 IP 匹配不足以重写
凭据，否则同一台电脑上跑两个 MyKVM 实例会互相顶掉配对记录。

### 7. 安卓端日志与一键上传

- `diag/Diag.kt` 写 `filesDir/diagnostics/mykvm-android.log`（512KB 轮转），
  Rust 侧的 `TeeLogger` 把同样的记录同时写进这个文件，所以一份日志里既有 Kotlin
  的注入流程，也有协议层的 discovery/QUIC 记录。
- 界面底部「诊断日志」卡片：**发送到电脑**（走已配对的 QUIC stream，
  `mykvm.diagnostics.v1`，电脑端存到自己的日志目录）、查看 / 刷新、分享
  （FileProvider）、清空。
- 排查时最快的路径仍然是 `adb logcat -s mykvm-core MyKvmDiag MyKvmService`。

### 8. 后台保活

前台服务在激进 ROM 上仍可能被冻结或杀掉，所以：

- 界面提供「加入白名单」按钮（`ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`）；
- `KeepAlive` 用 `setAndAllowWhileIdle` 每 9 分钟发一次广播，
  `KeepAliveReceiver` 在服务该运行时把它拉起来（重启后 `BOOT_COMPLETED` 同理）；
- 服务内部的 watchdog 每 15 秒确认 poll 线程还活着，死了就原地重启。

`KvmService.start()` 会先发通知再判断"是否已在运行"，因为从闹钟/开机广播经
`startForegroundService` 进来时，即使接收端没停也必须立刻进入前台，否则系统会抛
`ForegroundServiceDidNotStartInTimeException`。

### 9. 键盘：默认不要关平板输入法

`键盘直通`（`ImeSuppressor`）会把 `DEFAULT_INPUT_METHOD` 指向一个不存在的组件，
让平板**完全没有输入法**。它曾经是默认开启的，副作用非常严重：

- 平板上点输入框不弹软键盘，也没法切换输入法、打不了中文；
- 用户反馈的"键盘还是不能用"多数就是这个状态造成的。

**它也不再是必需的**：注入用的是 `KeyCharacterMap.getEvents()` 生成、
带真实字符的按键事件，真机上（默认输入法 = 搜狗）电脑端按键可以正常落到应用里；
中文模式下还会直接进入拼音候选（截图验证 `k`、`l` → 候选"快乐 / 看了 / 考虑"）。
所以：

- `keyboardPassthrough` 默认 **false**，`Prefs.migrateKeyboardPassthroughDefault()`
  会在升级后强制关闭一次，避免老安装继续把平板键盘关掉；
- 只有某些 ROM/输入法确实吞掉注入按键时，才让用户手动打开这一项；
- `MyKvmApp` 启动时的 `ImeSuppressor.repairIfNeeded()` 仍然保留：上一次运行
  留下 `mykvm/no-ime` 时会自动把真正的输入法放回去。

排查键盘问题最快的入口是应用内「诊断日志」，键事件会记录成：

```
key vk=0x4B -> keyCode=39 down=true meta=0x0 injected=true (#3)
```

`injected=false` 说明 Shizuku 注入被拒；完全没有这一行说明电脑端根本没有转发
按键（电脑端日志会给出原因，见下一节）。

### 10. 电脑端为什么不转发按键

Windows 的键盘钩子只在**控制权在远端**时才转发：`context.active` 为空时按键
原样留给本机。所以"鼠标能动、键盘不动"通常是控制权已经回到本机（或者从没过去）。
电脑端的日志现在会明确写出来：

```
keyboard: forwarding vk=0x4B down=true to peer-android-...
keyboard: key vk=0x4B was not forwarded -- no remote screen has control
```

另外可以用 `alt+→/←/↑/↓`（默认屏幕切换快捷键）直接把控制权切到平板，不需要
用鼠标划过屏幕边缘。

## 六、真机验证状态

已在 **Lenovo TB371FC（Android 14 / API 34 / arm64-v8a）** 上完成端到端验证：

| 项目 | 结果 |
| --- | --- |
| UDP 发现（47833） | ✅ 探测即时得到 `reply` |
| 配对握手 | ✅ 验证码显示 → QUIC `pair-confirm` 被接受并 ack |
| QUIC 传输（47834） | ✅ TLS 1.3 连接 + 证书钉扎 |
| 输入鉴权 | ✅ `first input packet accepted`；未配对的第三方探测被静默忽略 |
| **鼠标模式 · 光标** | ✅ 悬浮光标跨应用显示，坐标像素级准确 |
| **鼠标模式 · 左键** | ✅ 打开系统设置的蓝牙页 |
| **鼠标模式 · 滚轮** | ✅ 应用列表按真滚轮滚动 |
| **鼠标模式 · 右键** | ✅ 便签正文弹出上下文菜单（剪切/复制/粘贴/全选…） |
| **触摸模式 · 点击** | ✅ 打开系统设置的应用信息页 |
| **触摸模式 · 滚动** | ✅ 应用列表滑动生效 |
| **按键注入** | ✅ 注入 BACK 键使应用退回桌面 |
| 模式切换 | ✅ 界面即时生效并持久化，切换时释放按住的按键 |

仓库里带了两个可复用的验证工具：

```bash
# 在 Android 设备上跑服务后，从电脑侧驱动它（模拟桌面端）
cd android/rust
cargo run --example live_controller -- pair-request <平板IP>
cargo run --example live_controller -- pair-confirm <平板IP> <屏幕上的验证码>
cargo run --example live_controller -- trace <平板IP>       # 扫一遍光标
cargo run --example live_controller -- key <平板IP> 0xA6    # 注入 BACK 键
cargo run --example live_controller -- rclick <平板IP>       # 右键
cargo run --example live_controller -- scroll <平板IP>       # 滚轮

# 无设备时的快速连通性冒烟测试（仅对未配对的接收端有效）
MYKVM_LIVE_PEER=<平板IP>:47833 cargo test -- --nocapture live_probe
```

## 七、构建

### 依赖

- JDK 17
- Android SDK：`platforms;android-35`、`build-tools;35.0.0`
- Android NDK：`27.2.12479018`
- Rust 目标：`aarch64-linux-android`（真机）、`x86_64-linux-android`（模拟器）

```bash
rustup target add aarch64-linux-android x86_64-linux-android
```

### local.properties

```properties
sdk.dir=/path/to/android-sdk
ndk.dir=/path/to/android-sdk/ndk/27.2.12479018
```

（也可以改用环境变量 `ANDROID_HOME` + `ANDROID_NDK_HOME`。）

### 构建

```bash
cd android
./gradlew :app:assembleDebug        # 产物：app/build/outputs/apk/debug/app-debug.apk
./gradlew :app:assembleRelease      # R8 混淆 + 资源压缩
```

Gradle 会先调用 `:app:buildRustCore` 交叉编译 Rust 协议核心，把
`libmykvm_core.so` 放进 `app/src/main/jniLibs/<abi>/`，再打包。

只构建某一个 ABI（例如只出真机包，体积更小更快）：

```bash
./gradlew :app:assembleRelease -PrustAbis=arm64-v8a
```

默认 ABI 列表是 `arm64-v8a,x86_64`。

---

## 八、架构

```
android/
├── rust/                        # 协议核心（Rust → libmykvm_core.so）
│   └── src/
│       ├── quic_transport.rs    # 从桌面端 src-tauri/src 原样复制
│       ├── packet.rs            # 线格式类型，与桌面端逐字段对齐
│       ├── discovery.rs         # UDP 发现、广播、单播扫描
│       ├── pairing.rs           # 配对握手
│       ├── state.rs             # 接收端状态 + 注入鉴权
│       ├── receiver.rs          # 把传输层和发现层接起来
│       └── lib.rs               # JNI 导出
└── app/                         # Kotlin
    └── src/main/java/com/mykvm/receiver/
        ├── core/NativeCore.kt       # JNI 声明
        ├── KvmService.kt            # 前台服务、轮询、唤醒锁
        ├── input/ShizukuInjector.kt # injectInputEvent
        ├── input/InputDispatcher.kt # 协议事件 → 注入动作
        ├── input/KeyMap.kt          # Windows VK → Android KeyCode
        ├── input/VirtualCursor.kt   # 悬浮光标
        └── ui/ReceiverScreen.kt     # Compose 界面
```

### 职责边界

- **Rust**：线格式、发现、QUIC、鉴权。和桌面端共用同一套协议定义，
  不会漂移。`quic_transport.rs` 是**逐字节复制**的，桌面端改协议时这里要同步。
- **Kotlin**：Android 相关的一切 —— 注入、悬浮窗、权限、存储。

### 数据流

```
桌面端 --UDP 47833--> 发现/配对
桌面端 --QUIC 47834--> InputPacket(MessagePack)
                          |
                    Rust 解包 + 鉴权
                          |
                nativePoll() 返回 JSON
                          |
                    Kotlin 分发
                          |
              Shizuku → InputManager.injectInputEvent
```

---

## 九、已知限制

- **没有降级方案**：Shizuku 不可用时无法注入。曾考虑用
  `AccessibilityService` 兜底，但它只能点击/滑动，**无法注入键盘**，
  与被控端的核心需求不符，因此按精简方案移除。
- **重启后需重启 Shizuku**（无 root 的固有限制）。
- **鼠标模式在触摸专属 App 里可能没有反应**：右键和滚轮依赖 App 支持鼠标，
  不支持的就什么也不做。切到触摸模式即可（这正是那个开关的用途）。
- **HOME / 最近任务键无法注入**：AOSP 对这类系统按键有额外的注入限制，
  即使持有 `INJECT_EVENTS` 也会被静默忽略。Back / 音量 / 媒体键不受影响。
- **升级 Android 大版本后注入可能失效**：事务号表需要更新，见第五节。
- **横竖屏切换会重启接收服务**：屏幕尺寸上报给了桌面端，旋转后必须重新广播。
- 单屏，不支持多显示器/布局编辑（精简范围）。
- 不支持文件传输与剪贴板同步（精简范围）。

---

## 十、安全

与桌面端一致：**仅限可信局域网**。

- 发现通道（UDP 47833）是明文、未认证的。
- 输入通道走 QUIC/TLS 1.3，并钉扎对端广播的证书。
- 注入需要同时满足：`cluster_id` + `pair_secret` 匹配，且来源在
  `paired_controllers` 白名单内。
- 无凭证的「精简」数据包（稳态时每秒数百个鼠标移动）只在**该来源地址
  此前用完整凭证通过鉴权**后的短时间内被接受。

请勿把 47833/47834 端口暴露到公网。
