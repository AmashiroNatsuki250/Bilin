# 比邻组网控制台

控制台使用简体中文。访问首页登录后，管理员进入节点、设备和网络管理，普通用户进入自己的设备和网络管理。

## 开发

从仓库根目录运行。

```sh
pnpm install --frozen-lockfile
pnpm --dir bilin-web/frontend dev
```

开发代理默认连接 `http://localhost:11211`。通过 `API_BASE_URL` 可指定其他后端。

## 验证

```sh
pnpm --dir bilin-web/frontend exec playwright install chromium
pnpm --dir bilin-web/frontend test:browser
```

浏览器测试覆盖旧链接跳转、中文登录、角色分流、节点状态展示和用户创建网络。测试默认使用本地预览和模拟接口。通过 `WEB_TEST_URL` 可指定正在运行的前端。

下列测试使用新建的测试数据库、真实后端和运行在 `rust` 容器中的 Core。容器需要访问相同的仓库目录和虚拟网卡。

```sh
pnpm --dir bilin-web/frontend test:e2e
```
