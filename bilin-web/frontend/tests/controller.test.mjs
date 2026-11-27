import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { chromium } from 'playwright';

const base = process.env.WEB_TEST_URL || 'http://127.0.0.1:5198';
const deviceId = '00000000-0000-0000-0000-000000000001';
const deviceLoginTicket = 'd'.repeat(64);
let browser;
let server;

before(async () => {
    if (!process.env.WEB_TEST_URL) {
        server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', 'preview', '--port', '5198', '--strictPort']);
        for (let attempt = 0; attempt < 100; attempt++) {
            if (await fetch(base).then(response => response.ok).catch(() => false)) break;
            if (server.exitCode !== null || attempt === 99) throw new Error('Vite preview did not start');
            await delay(100);
        }
    }
    browser = await chromium.launch({ executablePath: process.env.WEB_TEST_CHROMIUM });
});

after(async () => {
    await browser?.close();
    server?.kill();
});

async function open(t, admin, registeredDevice = false, viewport, {
    hasNetwork = registeredDevice, deviceOnline = true, additionalNetwork = false, deviceLogin = false,
    unboundDevice = false,
} = {}) {
    const context = await browser.newContext({ baseURL: base, locale: 'en-US', viewport });
    t.after(() => context.close());
    await context.addInitScript(() => localStorage.setItem('lang', 'en'));
    const writes = [];
    const requests = [];
    let loggedIn = false;
    let network = hasNetwork;
    let ownedDevice = registeredDevice;
    let deleted = false;
    let configConnected = false;
    let proxyCidrs = [];
    let upstreamSubnetCidr = null;
    let registeredNode = false;
    let memberPresent = registeredDevice && hasNetwork;
    let secondMemberPresent = false;
    let rejectPolicy = false;
    let rejectDelete = false;
    let rejectMember = false;
    let deviceLoginApproved = false;
    let deviceName = '验收设备';
    let createdByDeviceLogin;
    const users = [
        { id: 2, username: 'tester', banned: false, udp_enabled: false, force_relay: false, forced_protocol: 'auto' },
        { id: 3, username: '普通用户', banned: false, udp_enabled: false, force_relay: false, forced_protocol: 'auto' },
        { id: 1, username: 'admin', banned: false, udp_enabled: false, force_relay: false, forced_protocol: 'auto' },
    ];
    const listedDevices = () => !deleted && (ownedDevice || admin && unboundDevice) ? [{
        device_id: deviceId, device_code: 'a123b456',
        hostname: deviceName, owner_id: ownedDevice ? 2 : null, online: deviceOnline, config_connected: configConnected, last_seen: 1795676400,
        assignments: memberPresent ? [{ network_id: 'test-network', display_name: '测试网', path: configConnected ? 'direct' : 'single_device', gateway: 'tcp://192.0.2.4:22021' }] : [],
    }] : [];
    const membersPath = admin ? '/api/v1/admin/users/2/networks/test-network/devices' : '/api/v1/networks/test-network/members';
    const boundMember = () => [{
        device_id: deviceId, hostname: deviceName, online: true, running: true,
        runtime_virtual_ipv4: '10.250.0.1/24', proxy_cidrs: proxyCidrs,
    }];
    const page = await context.newPage();
    page.setDefaultTimeout(8000);
    await page.route('**/api_meta.js', route => route.fulfill({
        contentType: 'text/javascript',
        body: 'window.apiMeta = { api_host: "" };',
    }));
    await page.route('**/api/v1/**', async route => {
        const request = route.request();
        const path = new URL(request.url()).pathname;
        const method = request.method();
        requests.push(path);
        if (method !== 'GET') writes.push({ path, method, data: request.postDataJSON() });
        if (path === '/api/v1/auth/login') {
            if (request.postDataJSON().username !== 'tester') {
                return route.fulfill({ status: 401, json: { message: 'Invalid credentials' } });
            }
            loggedIn = true;
            return route.fulfill({ json: {} });
        }
        if (deviceLogin && path === '/api/v1/enrollment/login/details') {
            if (deviceLoginApproved || request.postDataJSON().ticket !== deviceLoginTicket) {
                return route.fulfill({ status: 404 });
            }
            return route.fulfill({ json: { hostname: '待绑定的电脑', device_code: 'a123b456' } });
        }
        if (!loggedIn) return route.fulfill({ status: 401, json: {} });
        if (deviceLogin && path === '/api/v1/workspace/device-login') {
            assert.deepEqual(request.postDataJSON(), { ticket: deviceLoginTicket });
            assert.equal(deviceLoginApproved, false);
            deviceLoginApproved = true;
            ownedDevice = true;
            return route.fulfill({ status: 204 });
        }
        if (path === '/api/v1/console-info') {
            return route.fulfill({ json: { username: 'tester', platform_admin: admin } });
        }
        if (admin && path === '/api/v1/admin/users' && method === 'GET') {
            return route.fulfill({ json: { users } });
        }
        if (admin && path === '/api/v1/admin/users/3' && method === 'DELETE') {
            assert.equal(new URL(request.url()).searchParams.get('confirm'), 'true');
            if (rejectDelete) {
                rejectDelete = false;
                return route.fulfill({ status: 503 });
            }
            users.splice(1, 1);
            return route.fulfill({ status: 204 });
        }
        if (admin && path === '/api/v1/admin/users/3/policy' && method === 'PATCH') {
            if (rejectPolicy) {
                rejectPolicy = false;
                return route.fulfill({ status: 503, json: { message: 'Operation failed' } });
            }
            Object.assign(users[1], request.postDataJSON());
            return route.fulfill({ json: users[1] });
        }
        if (path === '/api/v1/admin/overview' && admin) {
            return route.fulfill({ json: {
                users: [{ id: 2, username: 'tester' }], networks: network ? [{
                    network_id: 'test-network', display_name: '测试网', networking_method: 'Gateway', owner_id: 2,
                }] : [], devices: listedDevices().map(device => ({ ...device, upstream_subnet_cidr: upstreamSubnetCidr })),
                gateway: null, gateway_forwarding: false,
                forwarding_nodes: [{ id: 'primary', name: '深圳主节点', peer_url: 'tcp://192.0.2.4:22021', online: true, ready: true, last_seen: 1795676400 },
                    ...(registeredNode ? [{ id: 'new-node', name: '备用节点', peer_url: 'tcp://192.0.2.5:22021', online: false, ready: false, last_seen: null }] : [])],
            } });
        }
        if (path === '/api/v1/admin/nodes' && method === 'POST') {
            registeredNode = true;
            return route.fulfill({ json: { id: 'new-node', registration_token: 'one-time-demo-token' } });
        }
        if (path === '/api/v1/admin/devices/a123b456' && method === 'DELETE') {
            assert.equal(new URL(request.url()).searchParams.get('confirm'), 'true');
            deleted = true;
            return route.fulfill({ status: 204 });
        }
        if (admin && path === `/api/v1/admin/devices/${deviceId}/upstream-subnet` && method === 'PATCH') {
            upstreamSubnetCidr = request.postDataJSON().upstream_subnet_cidr;
            return route.fulfill({ status: 204 });
        }
        if (path === `/api/v1/${admin ? 'admin' : 'workspace'}/devices/${deviceId}/name` && method === 'PATCH') {
            if (!listedDevices().length) return route.fulfill({ status: 404, json: { message: 'device not found' } });
            deviceName = request.postDataJSON().name;
            return route.fulfill({ status: 204 });
        }
        if (path === '/api/v1/workspace/devices') return route.fulfill({ json: listedDevices() });
        if (path === membersPath && method === 'GET') {
            return route.fulfill({ json: { members: memberPresent ? boundMember() : [] } });
        }
        if (additionalNetwork && path === '/api/v1/networks/second-network/members' && method === 'GET') {
            return route.fulfill({ json: { members: secondMemberPresent ? boundMember() : [] } });
        }
        if (additionalNetwork && path === '/api/v1/workspace/networks/second-network/devices' && method === 'POST') {
            secondMemberPresent = true;
            return route.fulfill({ status: 204 });
        }
        if (deviceLogin && path === '/api/v1/workspace/networks/bound-device-network/devices' && method === 'POST') {
            if (rejectMember) {
                rejectMember = false;
                return route.fulfill({ status: 503 });
            }
            return route.fulfill({ status: 204 });
        }
        if (path === '/api/v1/workspace/networks/test-network/devices' && method === 'POST') {
            if (rejectMember) {
                rejectMember = false;
                return route.fulfill({ status: 503 });
            }
            memberPresent = true;
            return route.fulfill({ status: 204 });
        }
        if (path === '/api/v1/workspace/networks/test-network/devices/by-code' && method === 'POST') {
            if (rejectMember) {
                rejectMember = false;
                return route.fulfill({ status: 503 });
            }
            if (request.postDataJSON().device_id !== 'a123b456') {
                return route.fulfill({ status: 404, json: { message: 'device not found' } });
            }
            ownedDevice = true;
            memberPresent = true;
            return route.fulfill({ status: 204 });
        }
        if (path === `${membersPath}/${deviceId}` && method === 'DELETE') {
            memberPresent = false;
            return route.fulfill({ status: 204 });
        }
        if (path === '/api/v1/workspace/networks' && method === 'POST') {
            if (deviceLogin) {
                createdByDeviceLogin = {
                    network_id: 'bound-device-network', display_name: request.postDataJSON().display_name,
                    networking_method: 'Gateway', secure_mode: true,
                };
                return route.fulfill({ json: { network_id: createdByDeviceLogin.network_id } });
            }
            network = true;
            return route.fulfill({ json: { network_id: 'test-network' } });
        }
        if (path === '/api/v1/workspace/devices/claim' && method === 'POST') {
            ownedDevice = true;
            return route.fulfill({ status: 204 });
        }
        if (path === '/api/v1/networks') {
            const listedNetworks = network ? [{
                network_id: 'test-network', display_name: '我的网络', networking_method: 'Gateway', secure_mode: true,
            }] : [];
            if (additionalNetwork) listedNetworks.push({
                network_id: 'second-network', display_name: '第二个网络', networking_method: 'Gateway', secure_mode: true,
            });
            if (createdByDeviceLogin) listedNetworks.push(createdByDeviceLogin);
            return route.fulfill({ json: { networks: listedNetworks } });
        }
        return route.fulfill({ status: 404, json: { message: 'Unexpected request' } });
    });
    await page.goto('/#/h/networks');
    await page.getByRole('heading', { name: '比邻组网' }).waitFor();
    return { page, writes, requests, connectConfig: () => { configConnected = true; },
        reportProxy: (ranges) => { proxyCidrs = ranges; }, rejectNextPolicy: () => { rejectPolicy = true; },
        rejectNextDelete: () => { rejectDelete = true; },
        rejectNextMember: () => { rejectMember = true; },
        reportForcedProtocol: (protocol) => { users[1].forced_protocol = protocol; } };
}

async function login(page, username = 'tester') {
    await page.getByLabel('用户名', { exact: true }).fill(username);
    await page.getByLabel('密码', { exact: true }).fill('test-password');
    await page.getByRole('button', { name: '登录', exact: true }).click();
}

async function navigate(page, label) {
    await page.getByRole('navigation', { name: '控制台导航' }).getByRole('link', { name: label, exact: true }).click();
}

test('旧页面进入中文登录，管理员直接看到节点服务器', async t => {
    const { page, requests } = await open(t, true);
    assert.equal(await page.title(), '比邻组网控制台');
    assert.equal(await page.locator('html').getAttribute('lang'), 'zh-CN');
    await login(page, 'wrong-user');
    await page.getByText('用户名或密码错误').waitFor();
    await login(page);
    await page.waitForURL('**/#/admin');
    await page.getByRole('heading', { name: '组网概览' }).waitFor();
    await navigate(page, '节点服务器');
    await page.getByRole('row', { name: /深圳主节点.*在线/ }).waitFor();
    assert.equal(await page.getByText('高级控制台').count(), 0);
    assert.equal(requests.some(path => /^\/api\/v1\/(machines|summary|sessions)$/.test(path)), false);
});

test('设备授权使用现有 Web 登录，成功后绑定且清除地址栏授权', async t => {
    const { page, writes } = await open(t, false, false, undefined, { deviceLogin: true });
    const authorizationUrl = `/#/device-login?ticket=${deviceLoginTicket}`;
    await page.goto(authorizationUrl);
    await page.getByText('待绑定的电脑', { exact: true }).waitFor();
    await page.getByText('a123b456', { exact: true }).waitFor();
    assert.equal(writes.filter(write => write.path === '/api/v1/workspace/device-login').length, 0);
    const submit = page.getByRole('button', { name: '登录并绑定本机', exact: true });
    await page.getByLabel('用户名', { exact: true }).fill('wrong-user');
    await page.getByLabel('密码', { exact: true }).fill('wrong-password');
    await submit.click();
    await page.getByText('用户名或密码错误').waitFor();
    assert.equal(writes.filter(write => write.path === '/api/v1/workspace/device-login').length, 0);
    await page.getByLabel('用户名', { exact: true }).fill('tester');
    await page.getByLabel('密码', { exact: true }).fill('test-password');
    await submit.click();
    await page.getByRole('heading', { name: '本机已绑定' }).waitFor();
    await page.waitForURL('**/#/device-login');
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => write.path), [
        '/api/v1/workspace/device-login',
    ]);
    const join = page.getByRole('dialog', { name: '加入网络' });
    await join.getByRole('textbox', { name: '网络名称' }).waitFor();
    await join.getByRole('button', { name: '稍后加入' }).click();
    await join.waitFor({ state: 'hidden' });
    await page.goto(authorizationUrl);
    await page.getByText('授权已失效，请回到客户端继续登录。').waitFor();
});

test('浏览器已有 Web 会话时展示当前帐号，点击绑定才使用授权', async t => {
    const { page, writes } = await open(t, false, false, undefined, { deviceLogin: true, hasNetwork: true });
    await login(page);
    await page.waitForURL('**/#/workspace');
    await page.goto(`/#/device-login?ticket=${deviceLoginTicket}`);
    await page.getByText('将这台设备绑定到账号「tester」。', { exact: true }).waitFor();
    assert.equal(writes.filter(write => write.path === '/api/v1/workspace/device-login').length, 0);
    await page.getByRole('button', { name: '切换账号' }).click();
    await page.getByLabel('用户名', { exact: true }).waitFor();
    assert.equal(writes.filter(write => write.path === '/api/v1/workspace/device-login').length, 0);
    await page.reload();
    await page.getByRole('button', { name: '绑定本机', exact: true }).click();
    await page.getByRole('heading', { name: '本机已绑定' }).waitFor();
    assert.equal(writes.filter(write => write.path === '/api/v1/workspace/device-login').length, 1);
    const join = page.getByRole('dialog', { name: '加入网络' });
    await join.getByText('是否将本机加入「我的网络」？').waitFor();
    await join.getByRole('button', { name: '创建网络' }).waitFor();
    assert.equal(await join.getByRole('combobox').count(), 0);
    await join.getByRole('button', { name: '加入网络', exact: true }).click();
    await page.getByText('已绑定到「tester」。本机已加入「我的网络」。', { exact: true }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/device-login', { ticket: deviceLoginTicket }],
        ['/api/v1/workspace/networks/test-network/devices', { device_id: deviceId }],
    ]);
});

test('设备授权后选中已有网络，只将刚绑定的本机加入目标网络', async t => {
    const { page, writes } = await open(t, false, false, undefined, {
        deviceLogin: true, hasNetwork: true, additionalNetwork: true,
    });
    await page.goto(`/#/device-login?ticket=${deviceLoginTicket}`);
    await page.getByLabel('用户名', { exact: true }).fill('tester');
    await page.getByLabel('密码', { exact: true }).fill('test-password');
    await page.getByRole('button', { name: '登录并绑定本机', exact: true }).click();
    const join = page.getByRole('dialog', { name: '加入网络' });
    const submit = join.getByRole('button', { name: '加入网络', exact: true });
    const select = join.getByRole('combobox', { name: '选择已有网络' });
    await select.waitFor();
    assert.ok(await submit.isDisabled());
    await select.click();
    await page.getByRole('option', { name: '第二个网络' }).click();
    await join.getByText('是否将本机加入「第二个网络」？').waitFor();
    await submit.click();
    await join.waitFor({ state: 'hidden' });
    await page.getByText('已绑定到「tester」。本机已加入「第二个网络」。', { exact: true }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/device-login', { ticket: deviceLoginTicket }],
        ['/api/v1/workspace/networks/second-network/devices', { device_id: deviceId }],
    ]);
});

test('设备授权弹窗可创建网络，加入失败后重试原网络', async t => {
    const { page, writes, rejectNextMember } = await open(t, false, false, undefined, {
        deviceLogin: true, hasNetwork: true,
    });
    await page.goto(`/#/device-login?ticket=${deviceLoginTicket}`);
    await page.getByLabel('用户名', { exact: true }).fill('tester');
    await page.getByLabel('密码', { exact: true }).fill('test-password');
    await page.getByRole('button', { name: '登录并绑定本机', exact: true }).click();
    const join = page.getByRole('dialog', { name: '加入网络' });
    await join.getByRole('button', { name: '创建网络' }).click();
    await join.getByRole('textbox', { name: '网络名称' }).fill('本机专网');
    rejectNextMember();
    await join.getByRole('button', { name: '创建并加入' }).click();
    await join.getByRole('alert').getByText('网络已创建。点击「加入网络」重试。').waitFor();
    await join.getByText('是否将本机加入「本机专网」？').waitFor();
    await join.getByRole('button', { name: '加入网络', exact: true }).click();
    await join.waitFor({ state: 'hidden' });
    await page.getByText('已绑定到「tester」。本机已加入「本机专网」。', { exact: true }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/device-login', { ticket: deviceLoginTicket }],
        ['/api/v1/workspace/networks', { display_name: '本机专网' }],
        ['/api/v1/workspace/networks/bound-device-network/devices', { device_id: deviceId }],
        ['/api/v1/workspace/networks/bound-device-network/devices', { device_id: deviceId }],
    ]);
});

test('首页可选同时添加设备，添加失败时重试原网络', async t => {
    const { page, writes, requests, rejectNextMember } = await open(t, false);
    await login(page);
    await page.waitForURL('**/#/workspace');
    await page.getByRole('heading', { name: '组网概览' }).waitFor();
    const guide = page.getByRole('dialog', { name: '新手引导' });
    await guide.getByRole('button', { name: '稍后继续' }).click();
    await guide.waitFor({ state: 'hidden' });
    await page.getByRole('button', { name: '创建网络', exact: true }).click();
    const starter = page.getByRole('dialog', { name: '创建网络', exact: true });
    const name = starter.getByRole('textbox', { name: '网络名称' });
    await name.fill('我的网络');
    assert.equal(await starter.getByRole('textbox', { name: '新设备编号' }).count(), 0);
    await starter.getByRole('checkbox', { name: '同时添加设备' }).check();
    const code = starter.getByRole('textbox', { name: '新设备编号' });
    const create = starter.getByRole('button', { name: '创建并加入' });
    await code.fill('a123b45');
    assert.equal(await create.isDisabled(), true);
    await code.fill(' A123B456 ');
    rejectNextMember();
    await create.click();
    const retry = page.getByRole('dialog', { name: '继续添加设备' });
    await retry.getByRole('alert').getByText(/网络已创建，设备添加未完成/).waitFor();
    await retry.getByRole('button', { name: '继续添加设备' }).click();
    await page.getByRole('heading', { name: '我的网络', exact: true }).waitFor();
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/networks', { display_name: '我的网络' }],
        ['/api/v1/workspace/networks/test-network/devices/by-code', { device_id: 'a123b456' }],
        ['/api/v1/workspace/networks/test-network/devices/by-code', { device_id: 'a123b456' }],
    ]);
    await page.goto('/#/admin');
    await page.waitForURL('**/#/workspace');
    assert.equal(requests.includes('/api/v1/admin/overview'), false);
    assert.equal(await page.getByRole('link', { name: '用户管理' }).count(), 0);
    await page.goto('/#/admin/users');
    await page.waitForURL('**/#/workspace');
    assert.equal(requests.includes('/api/v1/admin/users'), false);
});

test('首页勾选名下设备时可以建网并添加，设备离线可预先加入', async t => {
    const { page, writes } = await open(t, false, true, undefined, { hasNetwork: false, deviceOnline: false });
    await login(page);
    await page.getByRole('button', { name: '创建网络', exact: true }).click();
    const starter = page.getByRole('dialog', { name: '创建网络', exact: true });
    await starter.getByRole('textbox', { name: '网络名称' }).fill('我的网络');
    assert.equal(await page.getByRole('dialog', { name: '新手引导' }).count(), 0);
    await starter.getByRole('checkbox', { name: '同时添加设备' }).check();
    await starter.getByRole('checkbox', { name: /验收设备.*a123b456.*离线/ }).check();
    await starter.getByRole('button', { name: '创建并加入' }).click();
    await page.getByRole('heading', { name: '我的网络', exact: true }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => write.path), [
        '/api/v1/workspace/networks',
        '/api/v1/workspace/networks/test-network/devices',
    ]);
});

test('首页可单独绑定、单独建网或把设备加入已有网络', async t => {
    const { page, writes } = await open(t, false);
    await login(page);
    const guide = page.getByRole('dialog', { name: '新手引导' });
    await guide.getByRole('button', { name: '稍后继续' }).click();
    await guide.waitFor({ state: 'hidden' });
    assert.equal(await page.getByRole('textbox', { name: '网络名称' }).count(), 0);
    await page.getByRole('button', { name: '绑定设备', exact: true }).click();
    const bind = page.getByRole('dialog', { name: '绑定设备' });
    await bind.getByRole('textbox', { name: '设备编号' }).fill('a123b456');
    await bind.getByRole('button', { name: '绑定设备' }).click();
    await bind.waitFor({ state: 'hidden' });
    const workspaceWrites = () => writes.filter(write => write.path.startsWith('/api/v1/workspace/'));
    assert.deepEqual(workspaceWrites().map(write => write.path), ['/api/v1/workspace/devices/claim']);

    await page.getByRole('button', { name: '创建网络', exact: true }).click();
    const create = page.getByRole('dialog', { name: '创建网络' });
    await create.getByRole('textbox', { name: '网络名称' }).fill('我的网络');
    assert.equal(await create.getByRole('checkbox', { name: '同时添加设备' }).isChecked(), false);
    await create.getByRole('button', { name: '创建网络' }).click();
    await page.getByRole('heading', { name: '我的网络', exact: true }).waitFor();
    await page.getByText('暂无成员', { exact: true }).waitFor();

    await navigate(page, '组网概览');
    await page.getByRole('button', { name: '加入已有网络' }).click();
    const add = page.getByRole('dialog', { name: '添加设备', exact: true });
    await add.getByRole('combobox', { name: '选择已绑定的设备' }).click();
    await page.getByRole('option', { name: /验收设备.*a123b456/ }).click();
    await add.getByRole('button', { name: '加入网络' }).click();
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    assert.deepEqual(workspaceWrites().map(write => write.path), [
        '/api/v1/workspace/devices/claim',
        '/api/v1/workspace/networks',
        '/api/v1/workspace/networks/test-network/devices',
    ]);
});

test('有多个网络时从首页选择目标网络，再打开该网络的添加设备弹窗', async t => {
    const { page, writes } = await open(t, false, true, undefined, { additionalNetwork: true });
    await login(page);
    await page.getByRole('button', { name: '加入已有网络' }).click();
    const select = page.getByRole('dialog', { name: '加入已有网络' });
    await select.getByRole('combobox', { name: '选择网络' }).click();
    await page.getByRole('option', { name: '第二个网络' }).click();
    await select.getByRole('button', { name: '继续' }).click();
    await page.getByRole('heading', { name: '第二个网络' }).waitFor();
    await page.getByRole('dialog', { name: '添加设备' }).waitFor();
    assert.equal(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).length, 0);
});

test('五步引导箭头跟随当前步骤，回退后保留完成进度', async t => {
    const { page, writes, rejectNextMember } = await open(t, false);
    await login(page);
    const sidebar = page.getByRole('region', { name: '新手引导' });
    const guide = page.getByRole('dialog', { name: '新手引导' });
    await guide.getByRole('heading', { name: '下载客户端' }).waitFor();
    assert.equal(await guide.getByRole('link').count(), 0);
    assert.deepEqual(await sidebar.getByRole('listitem').allTextContents().then(items =>
        items.map(item => item.trim().replace(/^\d\s*/, ''))),
        ['下载客户端', '查询设备编号', '绑定设备', '选择网络', '加入网络']);
    assert.equal(await sidebar.getByRole('button', { name: '新手引导', exact: true }).count(), 0);
    await guide.getByRole('button', { name: '我已安装' }).click();
    await guide.getByText(/设备与控制端.*设备身份.*八位设备编号/).waitFor();
    assert.equal(await guide.getByText(/如果编号仍在获取中|确认客户端里的配置服务器/).count(), 0);
    await guide.getByRole('button', { name: '我已找到编号' }).click();
    await guide.getByRole('heading', { name: '绑定设备' }).waitFor();
    assert.equal(await sidebar.locator('li.done .pi-check').count(), 2);
    assert.equal(await sidebar.getByRole('button').count(), 1);
    assert.equal(await sidebar.getByRole('button', { name: '绑定设备', exact: true }).locator('.pi-chevron-right').count(), 1);
    await guide.getByRole('button', { name: '上一步' }).click();
    await guide.getByRole('heading', { name: '查询设备编号' }).waitFor();
    assert.equal(await sidebar.locator('li.done .pi-check').count(), 2);
    const findCodeStep = sidebar.getByRole('button', { name: '查询设备编号', exact: true });
    assert.equal(await findCodeStep.locator('.pi-chevron-right').count(), 1);
    await guide.getByRole('button', { name: '稍后继续' }).click();
    await guide.waitFor({ state: 'hidden' });
    await findCodeStep.click();
    await guide.getByRole('heading', { name: '查询设备编号' }).waitFor();
    await page.reload();
    await guide.getByRole('heading', { name: '查询设备编号' }).waitFor();
    assert.equal(await sidebar.locator('li.done .pi-check').count(), 2);
    await guide.getByRole('button', { name: '我已找到编号' }).click();
    await guide.getByRole('textbox', { name: '设备编号' }).fill(' A123B456 ');
    await guide.getByRole('button', { name: '绑定设备' }).click();
    await guide.getByRole('heading', { name: '选择网络' }).waitFor();
    await guide.getByRole('button', { name: '稍后继续' }).click();
    await guide.waitFor({ state: 'hidden' });
    await page.reload();
    const chooseStep = sidebar.getByRole('button', { name: '选择网络', exact: true });
    await chooseStep.waitFor();
    assert.equal(await guide.count(), 0);
    await chooseStep.click();
    await guide.getByRole('textbox', { name: '网络名称' }).fill('我的网络');
    await guide.getByRole('button', { name: '创建网络' }).click();
    await guide.getByRole('heading', { name: '加入网络' }).waitFor();
    await guide.getByRole('button', { name: '上一步' }).click();
    await guide.getByRole('combobox', { name: '选择已有网络' }).getByText('我的网络').waitFor();
    assert.equal(await guide.getByRole('textbox', { name: '网络名称' }).count(), 0);
    await guide.getByRole('button', { name: '继续', exact: true }).click();
    await guide.getByRole('heading', { name: '加入网络' }).waitFor();
    await page.reload();
    await sidebar.getByRole('button', { name: '加入网络', exact: true }).click();
    await guide.getByText('我的网络', { exact: true }).waitFor();
    rejectNextMember();
    const join = guide.getByRole('button', { name: '加入网络' });
    await join.click();
    await guide.getByRole('alert').waitFor();
    await join.click();
    await guide.getByRole('heading', { name: '设备已加入网络' }).waitFor();
    await guide.getByRole('button', { name: '完成' }).click();
    await page.getByRole('heading', { name: '我的网络', exact: true }).waitFor();
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/devices/claim', { device_id: 'a123b456' }],
        ['/api/v1/workspace/networks', { display_name: '我的网络' }],
        ['/api/v1/workspace/networks/test-network/devices', { device_id: 'a123b456' }],
        ['/api/v1/workspace/networks/test-network/devices', { device_id: 'a123b456' }],
    ]);
});

test('五步引导可选已有网络，换选后直接加入目标网络', async t => {
    const { page, writes } = await open(t, false, false, undefined, { hasNetwork: true, additionalNetwork: true });
    await login(page);
    const sidebar = page.getByRole('region', { name: '新手引导' });
    const guide = page.getByRole('dialog', { name: '新手引导' });
    await guide.getByRole('button', { name: '我已安装' }).click();
    await guide.getByRole('button', { name: '我已找到编号' }).click();
    await guide.getByRole('textbox', { name: '设备编号' }).fill('a123b456');
    await guide.getByRole('button', { name: '绑定设备' }).click();
    await guide.getByRole('heading', { name: '选择网络' }).waitFor();
    assert.equal(await guide.getByRole('textbox', { name: '网络名称' }).count(), 0);
    const network = guide.getByRole('combobox', { name: '选择已有网络' });
    const next = guide.getByRole('button', { name: '继续', exact: true });
    assert.ok(await next.isDisabled());
    await network.click();
    await page.getByRole('option', { name: '我的网络' }).click();
    await next.click();
    await guide.getByText('我的网络', { exact: true }).waitFor();
    await guide.getByRole('button', { name: '上一步' }).click();
    await network.click();
    await page.getByRole('option', { name: '第二个网络' }).click();
    await next.click();
    await guide.getByText('第二个网络', { exact: true }).waitFor();
    await page.reload();
    await sidebar.getByRole('button', { name: '加入网络', exact: true }).click();
    await guide.getByText('第二个网络', { exact: true }).waitFor();
    await guide.getByRole('button', { name: '加入网络' }).click();
    await guide.getByRole('heading', { name: '设备已加入网络' }).waitFor();
    await guide.getByRole('button', { name: '完成' }).click();
    await page.getByRole('heading', { name: '第二个网络' }).waitFor();
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/devices/claim', { device_id: 'a123b456' }],
        ['/api/v1/workspace/networks/second-network/devices', { device_id: 'a123b456' }],
    ]);
});

test('手机上的引导步骤在表单上方，关闭后仍可点下一步继续', async t => {
    const { page } = await open(t, false, false, { width: 390, height: 690 });
    await login(page);
    const guide = page.getByRole('dialog', { name: '新手引导' });
    await guide.getByRole('heading', { name: '下载客户端' }).waitFor();
    const rail = await guide.getByRole('list', { name: '组网步骤' }).boundingBox();
    const body = await guide.getByRole('heading', { name: '下载客户端' }).boundingBox();
    assert.ok(rail && body && rail.y + rail.height < body.y);
    await guide.getByRole('button', { name: '关闭新手引导' }).click();
    await guide.waitFor({ state: 'hidden' });
    const sidebar = page.getByRole('region', { name: '新手引导' });
    await sidebar.getByRole('button', { name: '下载客户端' }).click();
    await guide.getByRole('heading', { name: '下载客户端' }).waitFor();
});

test('关闭未开始的引导并从设备管理绑定后，侧栏收起引导', async t => {
    const { page } = await open(t, false);
    await login(page);
    const sidebar = page.getByRole('region', { name: '新手引导' });
    const guide = page.getByRole('dialog', { name: '新手引导' });
    await guide.getByRole('button', { name: '稍后继续' }).click();
    await guide.waitFor({ state: 'hidden' });
    await navigate(page, '设备管理');
    await sidebar.waitFor();
    await page.getByRole('button', { name: '绑定设备', exact: true }).click();
    const bind = page.getByRole('dialog', { name: '绑定设备' });
    await bind.getByRole('textbox', { name: '设备编号' }).fill('a123b456');
    await bind.getByRole('button', { name: '绑定设备' }).click();
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    await sidebar.waitFor({ state: 'hidden' });
});

test('用户从网络详情输入新设备编号，一次请求完成绑定和入网', async t => {
    const { page, writes } = await open(t, false, true);
    await login(page);
    await navigate(page, '网络管理');
    await page.getByRole('link', { name: /我的网络/ }).click();
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    await page.getByRole('button', { name: '添加设备', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: '添加设备', exact: true });
    const code = dialog.getByRole('textbox', { name: '设备编号' });
    const join = dialog.getByRole('button', { name: '加入网络' });
    await code.fill('bad00000');
    await join.click();
    await dialog.getByRole('alert').getByText('无法添加此设备，请核对编号和设备状态').waitFor();
    await code.fill(' A123B456 ');
    await join.click();
    await dialog.waitFor({ state: 'hidden' });
    assert.deepEqual(writes.filter(write => write.path.startsWith('/api/v1/workspace/')).map(write => [write.path, write.data]), [
        ['/api/v1/workspace/networks/test-network/devices/by-code', { device_id: 'bad00000' }],
        ['/api/v1/workspace/networks/test-network/devices/by-code', { device_id: 'a123b456' }],
    ]);
});

function userActionsTrigger(page, username) {
    return page.getByRole('button', { name: `${username} 的更多操作`, exact: true });
}

async function openUserActions(page, username) {
    await userActionsTrigger(page, username).click();
    const menu = page.getByRole('menubar', { name: `${username} 的更多操作`, exact: true });
    await menu.waitFor();
    return menu;
}

test('管理员从更多菜单设置强制策略，恢复默认保留智能协议，封号独立', async t => {
    const { page, writes, rejectNextPolicy, reportForcedProtocol } = await open(t, true);
    await login(page);
    await navigate(page, '用户管理');
    await page.waitForURL('**/#/admin/users');
    await page.getByRole('heading', { name: '用户管理', exact: true }).waitFor();
    const user = page.getByRole('row', { name: /普通用户/ });
    const smart = user.getByRole('checkbox', { name: '普通用户 的智能协议' });
    const restore = user.getByRole('button', { name: '恢复默认', exact: true });
    const path = '/api/v1/admin/users/3/policy';
    const ownMenu = await openUserActions(page, 'tester');
    assert.equal(await ownMenu.getByRole('menuitem', { name: '封号', exact: true }).isDisabled(), true);
    await page.keyboard.press('Escape');
    await ownMenu.waitFor({ state: 'hidden' });
    assert.equal(await user.getByRole('button', { name: '强制中转', exact: true }).count(), 0);
    assert.equal(await user.getByRole('combobox').count(), 0);
    const openControls = () => openUserActions(page, '普通用户');
    async function chooseProtocol(label) {
        const menu = await openControls();
        await menu.getByRole('menuitem', { name: '强制协议', exact: true }).click();
        const protocols = menu.getByRole('menu', { name: '强制协议', exact: true });
        assert.deepEqual(await protocols.getByRole('menuitem').allTextContents(), ['UDP', 'KCP', 'QUIC', 'TCP', 'WSS']);
        await protocols.getByRole('menuitem', { name: label, exact: true }).click();
        await menu.waitFor({ state: 'hidden' });
    }
    async function assertRoutingActionsHidden() {
        const menu = await openControls();
        assert.equal(await menu.getByRole('menuitem', { name: '强制协议', exact: true }).count(), 0);
        assert.equal(await menu.getByRole('menuitem', { name: '强制中转', exact: true }).count(), 0);
        await page.keyboard.press('Escape');
        await menu.waitFor({ state: 'hidden' });
    }
    assert.equal(await smart.isChecked(), false);
    await smart.click();
    await user.getByText('智能协议', { exact: true }).waitFor();
    assert.equal(await smart.isChecked(), true);
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { udp_enabled: true } });

    reportForcedProtocol('faketcp');
    await page.reload();
    await user.getByText('强制协议已不可用', { exact: true }).waitFor();
    await assertRoutingActionsHidden();
    await restore.click();
    await smart.waitFor();
    assert.equal(await smart.isChecked(), true);

    rejectNextPolicy();
    await chooseProtocol('QUIC');
    await page.getByRole('alert').waitFor();
    assert.equal(await smart.isChecked(), true);
    await chooseProtocol('KCP');
    await user.getByText('强制协议 · KCP', { exact: true }).waitFor();
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { forced_protocol: 'kcp' } });
    assert.equal(await smart.count(), 0);
    await assertRoutingActionsHidden();
    await page.reload();
    await user.getByText('强制协议 · KCP', { exact: true }).waitFor();
    await restore.click();
    await smart.waitFor();
    assert.equal(await smart.isChecked(), true);
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { force_relay: false, forced_protocol: 'auto' } });

    await (await openControls()).getByRole('menuitem', { name: '强制中转', exact: true }).click();
    await user.getByText('强制中转', { exact: true }).waitFor();
    await restore.waitFor();
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { force_relay: true } });
    assert.equal(await smart.count(), 0);
    await assertRoutingActionsHidden();

    async function openBan() {
        await (await openControls()).getByRole('menuitem', { name: '封号', exact: true }).click();
    }
    const confirmation = page.getByRole('dialog', { name: '封号' });
    await openBan();
    await confirmation.getByText(/名下设备会断开连接/).waitFor();
    await confirmation.getByRole('button', { name: '取消', exact: true }).click();
    assert.equal(writes.some(write => write.data?.banned === true), false);
    await openBan();
    await confirmation.getByRole('button', { name: '封号', exact: true }).click();
    await user.getByText('已封禁', { exact: true }).waitFor();
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { banned: true } });

    await page.reload();
    await user.getByText('已封禁', { exact: true }).waitFor();
    await restore.waitFor();
    await (await openControls()).getByRole('menuitem', { name: '解封', exact: true }).click();
    await user.getByText('正常', { exact: true }).waitFor();
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { banned: false } });
    await restore.click();
    await smart.waitFor();
    assert.equal(await smart.isChecked(), true);
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { force_relay: false, forced_protocol: 'auto' } });
    await smart.click();
    await user.getByText('智能协议（默认 TCP）', { exact: true }).waitFor();
    assert.deepEqual(writes.at(-1), { path, method: 'PATCH', data: { udp_enabled: false } });
    await chooseProtocol('WSS');
    await restore.click();
    await smart.waitFor();
    assert.equal(await smart.isChecked(), false);
    await navigate(page, '组网概览');
    await page.waitForURL('**/#/admin');
});

test('管理员从更多菜单确认后删除用户，当前及内置管理员受保护', async t => {
    const { page, writes, rejectNextDelete } = await open(t, true);
    await login(page);
    await navigate(page, '用户管理');
    const ownTrigger = userActionsTrigger(page, 'tester');
    await ownTrigger.focus();
    await page.keyboard.press('Enter');
    const ownMenu = page.getByRole('menubar', { name: 'tester 的更多操作', exact: true });
    const ownDelete = ownMenu.getByRole('menuitem', { name: '删除用户', exact: true });
    assert.equal(await ownDelete.isDisabled(), true);
    assert.equal(await ownTrigger.getAttribute('aria-expanded'), 'true');
    await page.keyboard.press('Escape');
    await ownMenu.waitFor({ state: 'hidden' });
    assert.equal(await ownTrigger.getAttribute('aria-expanded'), 'false');
    assert.equal(await ownTrigger.evaluate(button => document.activeElement === button), true);

    const builtInMenu = await openUserActions(page, 'admin');
    const builtInDelete = builtInMenu.getByRole('menuitem', { name: '删除用户', exact: true });
    assert.equal(await builtInDelete.isDisabled(), true);
    await page.keyboard.press('Escape');
    await builtInMenu.waitFor({ state: 'hidden' });

    const user = page.getByRole('row', { name: /普通用户/ });
    assert.equal(await user.getByRole('button', { name: '删除用户' }).count(), 0);
    async function selectDeleteUser() {
        const menu = await openUserActions(page, '普通用户');
        await menu.getByRole('menuitem', { name: '删除用户', exact: true }).click();
    }
    const dialog = page.getByRole('dialog', { name: '删除用户' });
    await selectDeleteUser();
    await dialog.getByText(/普通用户.*名下网络将被永久删除.*解除绑定/).waitFor();
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    assert.equal(writes.some(write => write.method === 'DELETE'), false);

    rejectNextDelete();
    await selectDeleteUser();
    const confirm = dialog.getByRole('button', { name: '删除用户' });
    await confirm.click();
    await dialog.getByRole('alert').getByText('操作失败').waitFor();
    assert.equal(await user.isVisible(), true);
    await confirm.click();
    await dialog.waitFor({ state: 'hidden' });
    await user.waitFor({ state: 'hidden' });
    assert.deepEqual(writes.filter(write => write.method === 'DELETE').map(write => write.path),
        ['/api/v1/admin/users/3', '/api/v1/admin/users/3']);
});

test('管理员看到八位编号，确认后才删除已入网设备', async t => {
    const { page, writes, connectConfig } = await open(t, true, true);
    await login(page);
    await navigate(page, '设备管理');
    const row = page.getByRole('row', { name: /验收设备.*a123b456/ });
    await row.getByText('登记在线', { exact: true }).waitFor();
    await row.getByText('配置通道连接中', { exact: true }).waitFor();
    await row.getByText('分配的转发节点 深圳主节点', { exact: true }).waitFor();
    assert.equal(await row.getByText('设备间路径', { exact: false }).count(), 0);
    connectConfig();
    await row.getByText('配置通道已连接', { exact: true }).waitFor({ timeout: 4_000 });
    await row.getByText('设备间路径 · 直连', { exact: true }).waitFor();
    await row.getByRole('button', { name: '删除设备' }).click();
    const dialog = page.getByRole('dialog', { name: '删除设备' });
    await dialog.getByText(/退出所有网络/).waitFor();
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    assert.equal(writes.some(request => request.method === 'DELETE'), false);
    await row.getByRole('button', { name: '删除设备' }).click();
    await dialog.getByRole('button', { name: '删除设备', exact: true }).click();
    await row.waitFor({ state: 'hidden' });
    assert.equal(writes.at(-1).method, 'DELETE');
    assert.equal(writes.at(-1).path, '/api/v1/admin/devices/a123b456');
});

for (const admin of [true, false]) {
    test(admin ? '管理员可以修改未绑定设备的名称' : '用户可以修改自己的设备名称并在网络看到新名称', async t => {
        const { page, writes } = await open(t, admin, !admin, undefined, { unboundDevice: admin });
        await login(page);
        await navigate(page, '设备管理');
        const row = page.getByRole('row').filter({ hasText: 'a123b456' });
        await row.getByRole('button', { name: '修改设备名称' }).click();
        const dialog = page.getByRole('dialog', { name: '修改设备名称' });
        const name = dialog.getByRole('textbox', { name: '设备名称' });
        const save = dialog.getByRole('button', { name: '保存' });
        assert.equal(await name.inputValue(), '验收设备');
        assert.equal(await save.isDisabled(), true);
        await name.fill('   ');
        assert.equal(await save.isDisabled(), true);
        await name.fill('🙂'.repeat(33));
        await save.click();
        await dialog.getByRole('alert').getByText(/32 个字符/).waitFor();
        assert.equal(writes.some(request => request.method === 'PATCH'), false);

        const updatedName = admin ? '控制台测试设备' : '🙂'.repeat(32);
        await name.fill(`  ${updatedName}  `);
        await save.click();
        await dialog.waitFor({ state: 'hidden' });
        await row.getByText(updatedName, { exact: true }).waitFor();
        assert.deepEqual(writes.at(-1), {
            path: `/api/v1/${admin ? 'admin' : 'workspace'}/devices/${deviceId}/name`,
            method: 'PATCH',
            data: { name: updatedName },
        });
        await page.reload();
        await row.getByText(updatedName, { exact: true }).waitFor();
        if (!admin) {
            await navigate(page, '网络管理');
            await page.getByRole('link', { name: /我的网络/ }).click();
            await row.getByText(updatedName, { exact: true }).waitFor();
        }
    });
}

test('管理员可以为设备保存和清除上游网段', async t => {
    const { page, writes } = await open(t, true, true);
    await login(page);
    await navigate(page, '设备管理');
    const row = page.getByRole('row', { name: /验收设备.*a123b456/ });
    const openSettings = row.getByRole('button', { name: '设置上游网段' });
    await openSettings.click();
    const dialog = page.getByRole('dialog', { name: '设置上游网段' });
    const subnet = dialog.getByRole('textbox', { name: '上游网段' });
    const save = dialog.getByRole('button', { name: '保存' });
    await subnet.fill('10.168.10.5/24');
    await save.click();
    const invalidMessage = dialog.getByRole('alert');
    await invalidMessage.waitFor();
    assert.match(await invalidMessage.textContent(), /请输入有效的私有 IPv4 网段/);
    assert.equal(writes.some(request => request.method === 'PATCH'), false);

    await subnet.fill('10.168.10.0/24');
    await save.click();
    await dialog.waitFor({ state: 'hidden' });
    await row.getByText('10.168.10.0/24', { exact: true }).waitFor();
    const path = `/api/v1/admin/devices/${deviceId}/upstream-subnet`;
    assert.deepEqual(writes.at(-1), {
        path, method: 'PATCH', data: { upstream_subnet_cidr: '10.168.10.0/24' },
    });

    await openSettings.click();
    assert.equal(await subnet.inputValue(), '10.168.10.0/24');
    await subnet.clear();
    await save.click();
    await dialog.waitFor({ state: 'hidden' });
    await row.getByText('10.168.10.0/24', { exact: true }).waitFor({ state: 'hidden' });
    assert.deepEqual(writes.at(-1), {
        path, method: 'PATCH', data: { upstream_subnet_cidr: null },
    });
});

test('管理员在网络成员页设置上游网段，刷新仍显示已保存状态', async t => {
    const { page, writes, reportProxy } = await open(t, true, true);
    reportProxy(['192.168.100.0/24']);
    await login(page);
    await navigate(page, '网络管理');
    await page.getByRole('link', { name: /测试网/ }).click();
    const row = page.getByRole('row', { name: /验收设备.*a123b456/ });
    const openSettings = row.getByRole('button', { name: '设置上游网段' });
    await openSettings.click();
    const dialog = page.getByRole('dialog', { name: '设置上游网段' });
    const subnet = dialog.getByRole('textbox', { name: '上游网段' });
    await subnet.fill('10.168.10.0/24');
    await dialog.getByRole('button', { name: '保存' }).click();
    await dialog.waitFor({ state: 'hidden' });
    assert.deepEqual(writes.at(-1), {
        path: `/api/v1/admin/devices/${deviceId}/upstream-subnet`,
        method: 'PATCH',
        data: { upstream_subnet_cidr: '10.168.10.0/24' },
    });

    await page.reload();
    const pending = row.getByText(/上游网段已保存.*10\.168\.10\.0\/24.*尚未生效/);
    await pending.waitFor();
    assert.deepEqual(await row.locator('.member-subnet').allTextContents(), ['192.168.100.0/24']);
    await openSettings.click();
    assert.equal(await subnet.inputValue(), '10.168.10.0/24');
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    await dialog.waitFor({ state: 'hidden' });

    reportProxy(['192.168.100.0/24', '10.168.10.0/24']);
    await page.getByRole('button', { name: '刷新', exact: true }).click();
    await pending.waitFor({ state: 'hidden' });
    await row.locator('.member-subnet').filter({ hasText: '10.168.10.0/24' }).waitFor();
    assert.equal(await row.getByText('10.168.10.0/24', { exact: true }).count(), 1);
});

for (const admin of [true, false]) {
    test(`${admin ? '管理员' : '网络所有者'}查看客户端上报的子网代理网段`, async t => {
        const { page, writes, reportProxy } = await open(t, admin, true);
        reportProxy(['192.179.1.0/24', '10.168.10.0/24']);
        await login(page);
        await navigate(page, '网络管理');
        await page.getByRole('link', { name: admin ? /测试网/ : /我的网络/ }).click();
        await page.getByRole('columnheader', { name: '子网代理网段' }).waitFor();
        await page.getByText('192.179.1.0/24', { exact: true }).waitFor();
        await page.getByText('10.168.10.0/24', { exact: true }).waitFor();
        assert.equal(await page.getByRole('button', { name: '设置上游网段' }).count(), admin ? 1 : 0);
        reportProxy([]);
        await page.getByRole('button', { name: '刷新', exact: true }).click();
        await page.getByText('192.179.1.0/24', { exact: true }).waitFor({ state: 'hidden' });
        await page.getByText('10.168.10.0/24', { exact: true }).waitFor({ state: 'hidden' });
        assert.equal(writes.some(request => request.method === 'PATCH'), false);
    });
}

test('手机宽度下筛选节点并查看一次性注册码', async t => {
    const { page, writes } = await open(t, true, false, { width: 390, height: 760 });
    await login(page);
    await navigate(page, '节点服务器');
    await page.getByRole('row', { name: /深圳主节点/ }).waitFor();
    const search = page.getByRole('searchbox');
    await search.fill('不存在的记录');
    await page.getByText('没有匹配的记录').waitFor();
    await search.clear();
    await page.getByRole('button', { name: '创建节点注册码' }).click();
    const dialog = page.getByRole('dialog', { name: '创建节点注册码' });
    await dialog.getByRole('textbox', { name: '节点名称' }).fill('备用节点');
    await dialog.getByRole('textbox', { name: '节点公开地址' }).fill('tcp://192.0.2.5:22021');
    await dialog.getByRole('button', { name: '创建节点注册码' }).click();
    await dialog.getByText('one-time-demo-token', { exact: true }).waitFor();
    assert.deepEqual(writes.at(-1), {
        path: '/api/v1/admin/nodes', method: 'POST', data: { name: '备用节点', peer_url: 'tcp://192.0.2.5:22021' },
    });
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click();
    await dialog.waitFor({ state: 'hidden' });
    assert.equal(await page.getByText('one-time-demo-token').count(), 0);
    await page.getByRole('button', { name: '离线', exact: true }).click();
    await page.getByRole('row', { name: /备用节点/ }).waitFor();
    assert.equal(await page.getByRole('row', { name: /深圳主节点/ }).count(), 0);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);
});

test('用户从弹窗绑定设备并管理网络成员', async t => {
    const { page, writes } = await open(t, false, true);
    await login(page);
    await navigate(page, '设备管理');
    await page.getByRole('row', { name: /验收设备.*a123b456/ }).waitFor();
    assert.equal(await page.getByRole('button', { name: '设置上游网段' }).count(), 0);
    await page.getByRole('button', { name: '绑定设备' }).click();
    const bind = page.getByRole('dialog', { name: '绑定设备' });
    const deviceNumber = bind.getByRole('textbox', { name: '设备编号' });
    assert.equal(await bind.getByRole('textbox').count(), 1);
    const submit = bind.getByRole('button', { name: '绑定设备' });
    await deviceNumber.fill('A123B45');
    assert.equal(await submit.isDisabled(), true);
    await deviceNumber.fill('A123B456');
    await submit.click();
    await bind.waitFor({ state: 'hidden' });
    assert.deepEqual(writes.at(-1), {
        method: 'POST', path: '/api/v1/workspace/devices/claim',
        data: { device_id: 'a123b456' },
    });

    await navigate(page, '网络管理');
    await page.getByRole('link', { name: /我的网络/ }).click();
    await page.getByRole('button', { name: '移出网络' }).click();
    const remove = page.getByRole('dialog', { name: '移出网络' });
    await remove.getByRole('button', { name: '移出网络' }).click();
    await page.getByText('暂无成员', { exact: true }).waitFor();
    assert.equal(writes.at(-1).method, 'DELETE');
    await page.getByRole('button', { name: '添加设备', exact: true }).click();
    const add = page.getByRole('dialog', { name: '添加设备', exact: true });
    const existingDeviceMode = add.getByRole('radio', { name: '已绑定设备' });
    const deviceCodeMode = add.getByRole('radio', { name: '新设备编号' });
    await existingDeviceMode.press('ArrowRight');
    assert.equal(await deviceCodeMode.isChecked(), true);
    await deviceCodeMode.press('ArrowLeft');
    assert.equal(await existingDeviceMode.isChecked(), true);
    await add.getByRole('combobox', { name: '选择已绑定的设备' }).click();
    await page.getByRole('option', { name: /验收设备.*a123b456/ }).click();
    await add.getByRole('button', { name: '加入网络' }).click();
    await page.getByRole('row', { name: /验收设备/ }).waitFor();
    assert.deepEqual(writes.at(-1), {
        method: 'POST', path: '/api/v1/workspace/networks/test-network/devices', data: { device_id: 'a123b456' },
    });
});
