# 自建比邻服务

比邻默认不配置公共中继和 STUN 服务。直连、局域网或单机测试可直接使用自己的监听地址。跨网段部署时，填写自己维护的中继与 STUN 地址。

## 中继与 STUN

在有公网可达地址的机器启动一个比邻 Core，在 TCP 或 UDP 11010 端口监听。下例使用 TCP，每台加入节点应使用自己的虚拟地址，并将 URI 换成实际中继。

```sh
bilin-core --network-name my-bilin-net --network-secret '<strong-secret>' \
  --listeners tcp://0.0.0.0:11010
```

```sh
bilin-core --network-name my-bilin-net --network-secret '<strong-secret>' \
  --ipv4 10.77.0.2 --peers tcp://relay.example.test:11010 \
  --stun-servers stun.example.test:3478
```

如果使用配置文件，对应的顶层字段为 `stun_servers`、`stun_servers_v6` 和 `tcp_stun_servers`。每个字段接受服务器地址列表。UDP 地址也可通过 `txt:stun.example.test` 指向由你维护的 DNS TXT 记录。仅部署 UDP STUN 时，将 `tcp_stun_servers` 显式设为 `[]`，避免用 UDP 地址探测 TCP。

## Web 管理

使用包含前端的 `bilin-web-embed` 发布包，或在构建时启用 `embed` 特性。其默认 Web 端口为 TCP 11211，配置接入端口为 UDP 22020。通过反向代理提供 HTTPS，将 Web 前端与 API 放在同一源。为分离部署的前端指定 `BILIN_API_HOST` 或配置其 `api_meta.js`，并在反向代理或 API 中配置相应的跨域访问。

若反向代理按路径放行，请将 `POST /api/v1/enrollment/login/start`、`POST /api/v1/enrollment/login/details` 和 `POST /api/v1/workspace/device-login` 转发到控制端。设备授权确认使用同源浏览器会话。可用公网地址向前两个接口发起无凭据的申请和无效票据查询，核对控制端分别返回带 JSON 正文的 401 和 404。

安卓应用内登录需要 HTTPS，并使用现有 `/api/v1/auth/captcha`、`/api/v1/auth/register` 和 `/api/v1/auth/login`。按路径放行时也要转发 `POST /api/v1/workspace/mobile/device`、`POST /api/v1/enrollment/mobile/status`、`POST /api/v1/enrollment/mobile/join` 和 `POST /api/v1/enrollment/mobile/devices`。控制端的注册开关同时适用于 Web 和安卓。

设备改名使用 `GET` 和 `POST /api/v1/enrollment/device-name`，Web 用户端和管理员端分别使用 `PATCH /api/v1/workspace/devices/{设备 ID}/name` 和 `PATCH /api/v1/admin/devices/{设备 ID}/name`。按路径放行的反向代理需转发这些接口。

```sh
bilin-web-embed --api-server-addr 127.0.0.1 \
  --config-server-host config.example.test --config-server-port 22020
```

将控制台的 HTTPS 地址填入客户端「配置服务器」，或用 `bilin-core --controller <控制台地址>` 启动。客户端上线后自动登记。可以在用户端输入八位设备编号绑定，或在客户端通过 Web 账号绑定，再选择要加入的网络。独立节点的注册流程见 [中央组网](central-controller.md)。

命令行客户端登记后，可以用 `bilin-cli device-name --controller <控制台地址> "客厅电脑"` 改名。省略名称会显示当前名称。若启动核心时自定义了 `--state-dir`，改名时也填写同一路径。

## 从旧部署迁移

1. 通过旧版 CLI 或控制台导出需要的 TOML 配置，并备份旧数据库。新 Web 服务会创建自己的 `bilin.db`。
2. 将导出的配置放入比邻的独立配置目录，修改旧的节点与 STUN 地址，并将环境变量前缀迁移到 `BILIN_`。
3. 在新网络中部署同版本的中继、节点和 Web 服务。比邻使用独立的握手与 Web RPC 标识。
4. 检查两台比邻设备之间的节点发现、中继与 Web 管理，再停用旧部署。原版节点可在另一套网络中继续使用旧配置。

原生应用采用 `io.github.amashironatsuki250.bilin`。独立维护的 iOS 和鸿蒙下游应用还需配置各自的应用身份、签名和所用 SDK。
