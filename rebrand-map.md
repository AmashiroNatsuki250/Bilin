# P0 品牌重塑对照表

基线为 `main` 的 `c97f36b8d40f3bde7a95cc9ec99fe95fe35a6ff0`。本文记录本地实施的命名与部署约定。尖括号中的服务地址由部署者提供。

## 「比邻」的 P0 决策

| 项目 | 决定 |
|---|---|
| 品牌 | 中文显示名「比邻」，英文显示名 `Bilin`，工程目录和命令用 `bilin` |
| 技术命名 | Rust 标识符 `bilin_*`，类型名 `Bilin*`，环境变量 `BILIN_*`，网卡前缀 `bl`，npm scope `@bilin` |
| 应用标识 | `io.github.amashironatsuki250.bilin`，JNI 扩展为 `io.github.amashironatsuki250.bilin.jni` |
| 发布 | 公开仓库 `AmashiroNatsuki250/Bilin`，迁移前仓库 `AmashiroNatsuki250/EasyTier_pro`，GHCR 镜像 `ghcr.io/amashironatsuki250/bilin`，本地项目根目录改为 `Bilin` |
| 服务 | STUN 和中继由部署者显式配置，Web 控制台默认访问同源 API。仓库文档提供自建配置方式 |
| 协议 | P4 启用独立网络协议和 SDK ABI，验证新旧节点隔离 |
| 旧版迁移 | 使用独立的应用标识和运行目录，按文档显式导入配置，不自动卸载旧服务 |
| Logo | P1 制作比邻的透明源图与各平台图标 |

执行顺序为 P0 确认对照表，P1 更新用户可见品牌，P2 改发布产物与包名，P3 改目录和内部标识，P4 切换服务与协议。P1 至 P4 分别提交和验证，P1、P2 仅在指定范围内替换。

## 品牌、构建与发布

| 项目 | 基线 | 新值草案 | 阶段 |
|---|---|---|---|
| 用户可见名称 | `EasyTier`、`Easytier` | `比邻`、`Bilin`。GUI 窗口标题、安装包显示名称和 Web 文案共用这套命名 | P1 |
| 名称的代码写法 | `easytier`、`easytier_*`、`EasyTier*`、`EASYTIER_*` | `bilin`、`bilin_*`、`Bilin*`、`BILIN_*`。`slug` 用于文件和包名，Rust 标识符用下划线 | P2–P3 |
| CLI 与发布产物 | `easytier-core`、`easytier-cli`、`easytier-web`、`easytier-web-embed`、`easytier-gui`、`easytier-mini` | `bilin-core`、`bilin-cli`、`bilin-web`、`bilin-web-embed`、`bilin-gui`、`bilin-mini` | P2 |
| GitHub 仓库和下载 | `github.com/EasyTier/EasyTier` 及其 Releases | `github.com/AmashiroNatsuki250/Bilin` 及其 Releases | P1–P2 |
| 安装包和镜像 | `easytier-linux-<arch>-<version>.zip`、`ghcr.io/easytier/easytier` | `bilin-linux-<arch>-<version>.zip`、`ghcr.io/amashironatsuki250/bilin`。安装器和 CI 使用一致的命名 | P2 |
| npm 包 | `easytier-gui`、`easytier-frontend*`、`@easytier/runtime`、`@easytier/browser`、`@easytier/cloudflare` 等 | `bilin-gui`、`bilin-frontend*`、`@bilin/runtime`、`@bilin/browser`、`@bilin/cloudflare` 等 | P2 |
| Cargo workspace 与目录 | `easytier`、`easytier-core`、`easytier-proto`、`easytier-contrib` 等 | `bilin`、`bilin-core`、`bilin-proto`、`bilin-contrib` 等 | P3 |
| Go 模块 | `github.com/easytier/easytier/easytier-go` | `github.com/AmashiroNatsuki250/Bilin/bilin-go` | P3 |
| 发布元数据 | 各 crate 的 `description`、`homepage`、`repository`、`authors` | `<项目描述>`、`<官网>`、`<仓库 URL>`、`<发布者署名>`。保留适用的许可证和版权声明 | P2 |
| Logo | Tauri 多平台图标、Android 启动图标、未连接托盘图标、Web Logo 与 favicon | `<正式原图路径>` 或 `<经确认的临时图标>`，原图为 1024×1024 透明 PNG 或 SVG | P1 |

## 应用标识与运行时

| 项目 | 基线 | 新值草案 | 阶段 |
|---|---|---|---|
| Tauri 与 Android | `com.kkrainbow.easytier` | `io.github.amashironatsuki250.bilin`，按 GitHub 命名空间派生。同步 Gradle、Kotlin 路径和应用标识 | P2–P3 |
| Android JNI | `com.easytier.jni`、`Java_com_easytier_jni_EasyTierJNI_*` | `io.github.amashironatsuki250.bilin.jni` 与对应的 JNI 导出符号。同步检查 Kotlin 调用方和 `exports.map` | P2–P3 |
| iOS | `easytier-ios` Rust 库及其 C ABI | `bilin-ios` 及对应 C ABI。下游 iOS App 和 Network Extension 的 Bundle ID、签名与配置由下游项目确认 | P2–P3 |
| 鸿蒙 | `easytier-ohrs` HAR 模块和包名、`libeasytier_ohrs.so` | `bilin-ohrs`、`libbilin_ohrs.so`。同步 ArkTS 下游消费方，另行确认下游应用 Bundle Name | P2–P3 |
| Magisk | `easytier_magisk` | `bilin_magisk` | P2 |
| 安装、服务与数据 | `/opt/easytier`、`easytier.service`、`easytier@.service`、`EasyTierService`、`EasyTierWatchDog`、`easytier.log`，以及各平台的配置、数据库和 socket 路径 | `/opt/bilin`、`bilin.service`、`bilin@.service`、`BilinService`、`BilinWatchDog`、`bilin.log`。确认旧路径的迁移方式 | P2 |
| 环境变量 | `ET_*`、`EASYTIER_*` | `BILIN_*`。按变量用途核对同名后缀，例如 Rust CLI 与 Cloudflare 示例里的 `NETWORK_NAME` 和 `NETWORK_SECRET` | P3 |
| 网卡前缀 | `et_`、`et`、`etlb` | `bl_`、`bl`、`bllb`。连同实例和随机后缀验证 Linux 最长 15 字节限制，同步 Web 的实例名识别逻辑 | P3 |
| User-Agent | `easytier/{version}`、`easytier-core` | `bilin/{version}`、`bilin-core` | P3 |

## 服务地址与兼容性

| 项目 | 基线 | 新值草案 | 阶段 |
|---|---|---|---|
| 官网和文档 | `easytier.cn` 文档链接 | `<官网>`、`<文档入口>`。删除旧链接时核对 README、GUI 和 Issue 模板 | P1 |
| STUN | `txt:stun.easytier.cn`、`txt:stun-v6.easytier.cn` | `txt:<stun-host>`、`txt:<stun-v6-host>`。切换前验证 DNS TXT 发现及所引用的 IPv4、IPv6 服务 | P4 |
| 配置服务 | `https://config-server.easytier.cn`、`udp://config-server.easytier.cn:22020` 示例 | `https://<config-host>`、`udp://<config-host>:<port>`。分别确认 Web API 和客户端接入端口 | P4 |
| 公共中继 | `tcp://public.easytier.top:11010` | `tcp://<relay-host>:<port>`，由部署者显式配置 | P4 |
| 上游依赖 | `EasyTier/thunk`、`EasyTier/kcp-sys` | `<fork-owner>/thunk`、`<fork-owner>/kcp-sys`。切换前核对新仓库中的提交。`EasyTier/easytier-mini` 是文档指向的独立发布源，另行确认是否接管该发布入口 | P4 |
| 网络协议 | `easytier-peerconn-noise`、`easytier secret proof`、`easytier-relay-noise`、`easytier-256bit-key`、`easytier-webclient-noise-v1` | 保持互通时保留现有值，并实测新旧节点互联。选择隔离时在 P4 确定版本和协议标识，并验证新旧节点无法互联 | P4 |
| WASI 与 Protobuf | `easytier_host`、`easytier_host_*`、`package easytier;`、`(easytier.flag)` | 按 P4 的兼容决策处理。若改名，同步更新 host、guest、代码生成和测试制品。网络互通验证与 SDK/ABI 验证分别执行 | P4 |

## 部署与迁移记录

1. 上表记录本地实施采用的品牌和发布名称。
2. 独立协议实施及新旧节点隔离验证安排在 P4。
3. STUN、配置服务和中继地址由部署者提供。
4. 迁移旧版时显式导出配置、核对节点地址并导入新实例。
5. P1 提交 Logo 原图和各平台生成物。

P1 至 P4 各自提交。变更发布或内部命名时重新生成受影响的锁文件与构建产物，并用基线常量检查确认 P4 之前的协议和 ABI 未受影响。
