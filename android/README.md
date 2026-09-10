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

## 四、操作映射

Android 没有右键，也没有系统光标，所以映射如下：

| 电脑操作 | Android 行为 |
|---|---|
| 移动鼠标 | 移动**应用自绘的悬浮光标**（不注入，零延迟） |
| 左键单击 | 在光标处点击 |
| 按住左键拖动 | 连续触摸拖拽 |
| 右键单击 | 长按（等同于打开上下文菜单） |
| 中键 | 返回键 |
| 鼠标侧键 | 返回（HOME / 最近任务受系统限制，无法注入） |
| 滚轮 | 在光标处滑动（见下文说明） |
| 键盘 | 直接注入 `KeyEvent`，修饰键（Ctrl/Alt/Shift/Win）有效 |

屏幕以**物理像素 + scale 1.0** 上报，所以桌面端传来的坐标就是注入坐标，没有换算。

### 关于滚轮

滚轮被实现成**短距离滑动**，而不是 `ACTION_SCROLL`。

原因是 `MotionEvent.setAxisValue()` 只允许给一个**本来就有该轴**的事件赋值，
而 `MotionEvent.obtain()` 创建的事件不含 `AXIS_VSCROLL` 位，所以无法在注入前
把滚动轴加上去。滑动是每一个 App 都能正确理解的方式，代价是滚动的惯性由
系统手势决定，不完全等于鼠标滚轮。

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

## 六、真机验证状态

已在 **Lenovo TB371FC（Android 14 / API 34 / arm64-v8a）** 上完成端到端验证：

| 项目 | 结果 |
| --- | --- |
| UDP 发现（47833） | ✅ 探测即时得到 `reply` |
| 配对握手 | ✅ 验证码显示 → QUIC `pair-confirm` 被接受并 ack |
| QUIC 传输（47834） | ✅ TLS 1.3 连接 + 证书钉扎 |
| 输入鉴权 | ✅ `first input packet accepted`，无凭证包被正确拒绝 |
| 虚拟光标 | ✅ 跨应用显示，坐标像素级准确 |
| **触摸注入** | ✅ 点击系统设置「屏幕刷新率」成功打开选择器 |
| **按键注入** | ✅ 注入 BACK 键使应用退回桌面 |

仓库里带了两个可复用的验证工具：

```bash
# 在 Android 设备上跑服务后，从电脑侧驱动它（模拟桌面端）
cd android/rust
cargo run --example live_controller -- pair-request <平板IP>
cargo run --example live_controller -- pair-confirm <平板IP> <屏幕上的验证码>
cargo run --example live_controller -- trace <平板IP>       # 扫一遍光标
cargo run --example live_controller -- key <平板IP> 0xA6    # 注入 BACK 键

# 无语设备时的快速连通性冒烟测试
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
- **滚轮不完全等于鼠标滚轮**（原因见上）。
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
