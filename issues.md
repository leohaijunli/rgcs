# MagGCS (rgcs-main) Issue 清单

遗留问题与待办（2026-10-10 重组）：已完成的问题压缩为索引（git 历史含全文），**未完成项在下方全文列出**。


## Resolved issues（已完成，共 43 条 — 全文见 git 历史）

### 进度总览（P0）
### #1 [P0][core] Heartbeat timeout is reset by any inbound traffic
### #2 [P0][core/app] Connected event lost: hub subscribes after the receiver is dropped
### #3 [P0][core] GCS never sends HEARTBEAT
### #4 [P0][app] `connect` returns success even when binding fails
### #5 [P0][core/app/frontend] Flight commands are fire-and-forget (no COMMAND_ACK)
### #6 [P0][core] Telemetry aggregation has no source filtering
### #7 [P0][core] Mission protocol retransmission timer is never wired
### #8 [P0][core] Upload: duplicate MISSION_REQUEST treated as failure
### #9 [P0][core/app] Download accepts MISSION_ITEM_INT addressed to other GCSs
### #10 [P1][core] `MissionItem` model is redundant and lossy
### #11 [P1][core] `MissionFrame` does not round-trip
### #12 [P1][frontend] `.plan` import crashes on ComplexItem
### #13 [P1][frontend] `.plan` export is not faithful
### #14 [P1][frontend] Add-waypoint button inserts at ~(0°, 0°) when nothing is selected
### #15 [P1][frontend/app] Mission sync state is unmodelled; no read-back verification
### #16 [P1][core] Per-field data age is not tracked
### #17 [P1][core] Error events delivered through `watch<Option<String>>` can be lost
### #18 [P2][arch] Move `MissionService` from app-tauri into `core`
### #19 [P2][arch] Replace polling pump with event-driven hub
### #20 [P2][arch] Introduce message routing in the connection layer
### #21 [P2][arch] Make app state multi-vehicle ready and `connect` atomic
### #22 [P2][security] Tighten Tauri configuration
### #23 [P2][ci] ts-rs binding sync check can never fail
### #24 [P2][ci] Workflow inconsistencies
### #25 [P2][test] Add a lossy fake-FC test harness for the mission protocol
### #26 [P2][docs] Documentation and ADR consistency
### #27 [P3][frontend] Show a clear "MOCK DATA" indicator when `isMock` is true
### #28 [P3][frontend] Telemetry channel will not scale to Phase 4 mag data
### #29 [P3][frontend] Split `MapView.tsx` (~17 KB) before adding survey layers
### #30 [P3][build] Trim Cesium assets for packaging
### #31 [P3][core] Device hotplug watcher polls every 500 ms and fails hard
### #32 [P3][frontend] UAV marker: 3D model, MAVLink-driven attitude, forward-only prediction
### #33 [P1][frontend] Upload keeps saying "Unsaved changes — not the FC plan"
### #34 [P0][core/frontend] Upload fails with `mission ack denied (type 3)` on a plan with a DO item
### #35 [P2][frontend] Tie lines look like spacing lines; the Layers panel is decorative
### #36 [P2][frontend] No height visualization per waypoint
### #37 [P1][frontend] The waypoint list says nothing while flying
### #38 [P1][frontend] Upload reports "FC mission differs from the uploaded plan" for a plan the FC holds
### #39 [P2][frontend] Selecting a waypoint does not show it on the map; height sticks float
### #40 [P2][app/devices] 串口列表不自动刷新
### #41 [P2][app/devices] 串口列表应只显示 ttyACM* / ttyUSB*
### #42 [P1][helper] udev 规则/权限自动安装助手未实现
### #43 [P2][frontend] 设置菜单应提供 UAV 3D 模型朝向的设置（pitch/roll/yaw 偏移）

## Open issues（未完成）

### O1 [P2][frontend] Logs 页是占位
Settings → Logs 目前只有一段说明文字。SD 日志拉取（MAVLink LOG 协议或
MAVFTP，`etc/logging/logger_topics.txt` 写入）已有设计讨论，未实现。

### O2 [P3][actuator] 报警音可选自定义 wav/mp3 文件路径
当前为 WebAudio 合成蜂鸣（内置，无依赖）。plan §9.1 里的"可选本机音频文件"
设置尚未做。

### O3 [P3][actuator] 曲线扩展
Live 曲线目前为 总场(Δmag) + ESC 电流/RPM。plan §6.2 的另外两组（指令 +
姿态；磁场偏移基准曲线组）与显示/隐藏单条曲线的开关未做。

### O4 [P2][app] 双链路转发仅限 Motor 测试窗口
SITL 副链路（`actuator_connect_sitl`）挂在 MotorTestState 上；若未来
Headless/主界面也要用副链路，需要把 SITL 连接提升到 AppState 通用层。

### O5 [P3][frontend] Data 视图移除后的遗留键
`terrain.dtm/dsm`、`panels.status/current` 等 i18n 键与 `VIEW_CONFIG.dockTab`
中 profile/log 常量仍保留（Phase 3b/5 内容门控用），P6 screenshot 基线
尚未重拍。

## 待核实项（先验证再决定是否开 issue）

- `mavlink` crate 0.17 的 UDP 监听是否设置 `SO_REUSEADDR`；`udpin` 的回复地址行为（抓包确认）。
- `MISSION_COUNT` 的 `mission_type` 等扩展字段默认值，与 QGC 上传的帧对比。
- PX4 对多个 GCS 同时读取任务时的行为（影响 #9 的具体修法）。
- `DO_PAUSE_CONTINUE` 在 PX4 v1.17 不支持（已实测 NACK）——已用 NAV_TAKEOFF/LAND 替代，关闭。

## Deferred（操作员指示暂缓）

### D1 GoPro 视频监视（motor-test-plan §14 G1–G4）
HERO8/9（OpenGoPro 1.0）client 模式，mDNS 发现，HTTP 控制（录制
start/stop），RTP→ffmpeg→HLS 预览，视频卡无流不渲染。**操作员指示：
需要时再启动。**

### D2 M0 台架实测清单（motor-test-plan §11，需要硬件）
- FC 端 `COM_MOT_TEST_EN=1`。
- 验证 `ACTUATOR_TEST` 语义（超时/是否要求未解锁/停转动作）与
  `ESC_STATUS` 推送速率。
- SITL 转发端到端：SITL 飞任务 → Realtime forward → 真机电机随动。
- 报警参数（阈值/去抖/迟滞/冷却/α）按台架数据标定。

### D3 操作员验证清单（软件已就绪，待确认）
- 飞行状态着色（重启应用后确认）。
- 模型朝向设置（Settings → Vehicle）与串口热插拔 + 权限修复按钮。
- 三源图层（已确认 ✓）。
