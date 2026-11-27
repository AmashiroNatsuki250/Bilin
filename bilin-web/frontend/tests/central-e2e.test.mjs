import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, readdir, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { chromium } from 'playwright';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const webBinary = join(repo, 'target/debug/bilin-web');
const coreBinary = join(repo, 'target/debug/bilin-core');
const cliBinary = join(repo, 'target/debug/bilin-cli');
const username = 'central-e2e';
const password = 'e2e-password';
const machineId = '00000000-0000-0000-0000-000000000002';
// Argon2id of the MD5 digest sent by the frontend for the test password.
const passwordHash = '$argon2id$v=19$m=4096,t=3,p=1$ZWFzeXRpZXItZTJlLXNhbHQ$FHbDjVElaTXuArgEEQrQrO4AdsnqNOcXGA5PFY8cG/s';

function docker(...args) {
    const result = spawnSync('docker', ['exec', 'rust', ...args], { encoding: 'utf8' });
    if (result.status !== 0) throw new Error(result.stderr || result.stdout);
    return result.stdout.trim();
}

function buildCoreForDocker() {
    docker('sh', '-c', 'cd "$1" && cargo build -p bilin --bins && chown -R "$2:$3" target',
        'e2e-build', repo, String(process.getuid()), String(process.getgid()));
}

function freePort() {
    return new Promise((resolvePort, reject) => {
        const server = createServer();
        server.once('error', reject);
        server.listen(0, '0.0.0.0', () => {
            const port = server.address().port;
            server.close(() => resolvePort(port));
        });
    });
}

async function waitFor(check, label, timeout = 30000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        const result = await check().catch(() => null);
        if (result) return result;
        await delay(250);
    }
    throw new Error(`Timed out waiting for ${label}`);
}

function startWeb(database, apiPort, configPort, gatewayHost) {
    const child = spawn(webBinary, [
        '--db', database,
        '--api-server-addr', '127.0.0.1',
        '--api-server-port', String(apiPort),
        '--config-server-protocol', 'tcp',
        '--config-server-port', String(configPort),
        '--gateway-peer-url', `tcp://${gatewayHost}:${configPort}`,
        '--console-log-level', 'error',
    ], { stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    for (const stream of [child.stdout, child.stderr]) {
        stream.on('data', chunk => { output = (output + chunk).slice(-12000); });
    }
    return { child, logs: () => output };
}

async function stopWeb(web) {
    web.child.kill('SIGINT');
    if (web.child.exitCode === null) {
        await Promise.race([new Promise(resolveExit => web.child.once('exit', resolveExit)), delay(3000)]);
    }
}

function startCore(coreConfig, gatewayHost, configPort) {
    const child = spawn('docker', ['exec', 'rust', 'sh', '-c', `
        echo $$ > ${coreConfig}/core.pid
        exec ${coreBinary} \
            --config-server tcp://${gatewayHost}:${configPort}/${username} \
            --machine-id ${machineId} --config-dir ${coreConfig} \
            --no-listener --hostname e2e-device --console-log-level error
    `], { stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    for (const stream of [child.stdout, child.stderr]) {
        stream.on('data', chunk => { output = (output + chunk).slice(-12000); });
    }
    return { child, logs: () => output };
}

function startCredentialCore(coreConfig, networkName, credential, peerUrl, hostname, rpcPort) {
    const child = spawn('docker', ['exec', 'rust', 'sh', '-c', `
        echo $$ > "$1/core.pid"
        exec "$2" --network-name "$3" --secure-mode --credential "$4" \
            -p "$5" --no-listener --no-tun --hostname "$6" \
            --rpc-portal "127.0.0.1:$7" --console-log-level error
    `, 'credential-core', coreConfig, coreBinary, networkName, credential, peerUrl, hostname, String(rpcPort)],
    { stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    for (const stream of [child.stdout, child.stderr]) {
        stream.on('data', chunk => { output = (output + chunk).slice(-12000); });
    }
    return { child, logs: () => output };
}

function credentialPeers(rpcPort) {
    return JSON.parse(docker(cliBinary, '--rpc-portal', `127.0.0.1:${rpcPort}`, '--output', 'json', 'peer'));
}

async function stopCore(core, coreConfig) {
    if (!core) return;
    const pid = await readFile(join(coreConfig, 'core.pid'), 'utf8').catch(() => null);
    if (pid) {
        try { docker('kill', '-INT', pid.trim()); } catch { /* Core already exited. */ }
    }
    if (core.child.exitCode === null) {
        await Promise.race([new Promise(resolveExit => core.child.once('exit', resolveExit)), delay(3000)]);
        if (core.child.exitCode === null) core.child.kill('SIGTERM');
    }
}

function seedUser(database) {
    const db = new DatabaseSync(database);
    try {
        const user = db.prepare('INSERT INTO users(username, password) VALUES (?, ?)')
            .run(username, passwordHash);
        const group = db.prepare("SELECT id FROM groups WHERE name = 'users'").get();
        db.prepare('INSERT INTO users_groups(user_id, group_id) VALUES (?, ?)')
            .run(user.lastInsertRowid, group.id);
    } finally {
        db.close();
    }
}

async function login(page) {
    await page.goto('/');
    await page.locator('#username').fill(username);
    await page.getByLabel('密码', { exact: true }).fill(password);
    await page.getByRole('button', { name: '登录', exact: true }).click();
    await page.getByRole('heading', { name: '组网概览' }).waitFor();
}

async function api(context, path) {
    const response = await context.request.get(`/api/v1${path}`);
    assert.equal(response.status(), 200, `${path}: ${await response.text()}`);
    return response.json();
}

function protoUuid(uuid) {
    const hex = uuid.replaceAll('-', '');
    return Object.fromEntries([0, 1, 2, 3].map(index =>
        [`part${index + 1}`, parseInt(hex.slice(index * 8, index * 8 + 8), 16)]));
}

async function nodeRpc(context, networkId, method) {
    const response = await context.request.post(`/api/v1/machines/${machineId}/proxy-rpc`, {
        data: {
            service_name: 'api.instance.PeerManageRpcService',
            method_name: method,
            payload: { instance: { id: protoUuid(networkId) } },
        },
    });
    assert.equal(response.status(), 200, await response.text());
    return response.json();
}

test('browser, Web API, database and Core agree on central network lifecycle', { timeout: 300000 }, async () => {
    buildCoreForDocker();
    const temp = await mkdtemp(join(repo, '.central-e2e-'));
    const database = join(temp, 'web.db');
    const coreConfig = join(temp, 'core');
    const credentialConfigs = [join(temp, 'credential-one'), join(temp, 'credential-two')];
    await mkdir(coreConfig);
    for (const config of credentialConfigs) await mkdir(config);
    const apiPort = await freePort();
    const configPort = await freePort();
    const credentialRpcPorts = [await freePort(), await freePort()];
    const gatewayHostname = (await readFile('/etc/hostname', 'utf8')).trim() || 'bilin-web-gateway';
    const base = `http://127.0.0.1:${apiPort}`;
    const route = docker('sh', '-c', 'ip route');
    const gatewayHost = route.match(/default via ([\d.]+)/)?.[1];
    assert.ok(gatewayHost, `Docker bridge gateway missing: ${route}`);
    docker(coreBinary, '--version');
    docker(cliBinary, '--version');
    let web = startWeb(database, apiPort, configPort, gatewayHost);
    let core;
    const credentialCores = [];
    let browser;
    try {
        await waitFor(async () => (await fetch(`${base}/api_meta.js`)).ok, 'Web server');
        seedUser(database);
        browser = await chromium.launch({ headless: true });
        const context = await browser.newContext({ baseURL: base });
        const page = await context.newPage();
        await login(page);

        core = startCore(coreConfig, gatewayHost, configPort);
        await waitFor(async () => {
            const { machines } = await api(context, '/machines');
            return machines.some(machine => machine.info.hostname === 'e2e-device');
        }, 'Core enrollment');
        const created = await context.request.post('/api/v1/networks', {
            data: { settings: { display_name: 'E2E Network', networking_method: 'Gateway', peer_urls: [], secure_mode: true } },
        });
        assert.equal(created.status(), 200, await created.text());
        const { network_id: networkId } = await created.json();
        const added = await context.request.post(`/api/v1/networks/${networkId}/members`, {
            data: { device_ids: [machineId] },
        });
        assert.equal(added.status(), 204, await added.text());
        await waitFor(async () => {
            const { members } = await api(context, `/networks/${networkId}/members`);
            return members.find(member => member.device_id === machineId && member.running);
        }, 'managed Core instance', 45000);
        await waitFor(async () => {
            return (await nodeRpc(context, networkId, 'list_peer')).peer_infos?.length > 0;
        }, 'Core peer connection', 15000);
        await waitFor(async () => {
            const routes = (await nodeRpc(context, networkId, 'list_route')).routes ?? [];
            return routes.length > 0;
        }, 'Core route through the Gateway', 15000);

        const network = await api(context, `/networks/${networkId}`);
        assert.equal(network.secure_mode, true);
        const issued = await context.request.post(`/api/v1/networks/${networkId}/credentials`, {
            data: { ttl_seconds: 3600, reusable: true, credential_id: 'e2e-shared' },
        });
        assert.equal(issued.status(), 200, await issued.text());
        const credential = await issued.json();
        const credentialPeerUrl = `tcp://${gatewayHost}:${configPort}`;
        for (const [index, hostname] of ['e2e-temp-a', 'e2e-temp-b'].entries()) {
            credentialCores.push(startCredentialCore(
                credentialConfigs[index], network.network_name, credential.credential_secret,
                credentialPeerUrl, hostname, credentialRpcPorts[index],
            ));
        }
        const onlinePeers = await waitFor(async () => {
            const { credentials } = await api(context, `/networks/${networkId}/credentials`);
            const peers = credentials.find(item => item.credential_id === 'e2e-shared')?.online_peers;
            return peers?.length === 2 ? peers : null;
        }, 'both credential peers online', 45000);
        assert.deepEqual(onlinePeers.map(peer => peer.hostname).sort(), ['e2e-temp-a', 'e2e-temp-b']);
        assert.equal(new Set(onlinePeers.map(peer => peer.peer_id)).size, 2);
        const { temporary_peers: memberPeers } = await api(context, `/networks/${networkId}/members`);
        assert.deepEqual(memberPeers.map(peer => peer.hostname).sort(), ['e2e-temp-a', 'e2e-temp-b']);
        await waitFor(async () => credentialRpcPorts.every(port =>
            credentialPeers(port).some(peer => peer.hostname === gatewayHostname)),
        'both credential Cores connected to Gateway');
        const revoked = await context.request.delete(`/api/v1/networks/${networkId}/credentials/e2e-shared`);
        assert.equal(revoked.status(), 204, await revoked.text());
        assert.deepEqual((await api(context, `/networks/${networkId}/members`)).temporary_peers, []);
        await waitFor(async () => credentialRpcPorts.every(port =>
            !credentialPeers(port).some(peer => peer.hostname === gatewayHostname)),
        'revoked Core peers disconnected from Gateway');
        await delay(6000);
        assert.ok(credentialRpcPorts.every(port =>
            !credentialPeers(port).some(peer => peer.hostname === gatewayHostname)),
        'revoked credential Cores must not reconnect to Gateway');
        assert.deepEqual((await api(context, `/networks/${networkId}/members`)).temporary_peers, [],
            'revoked credential peers must not reconnect');
        for (const [index, credentialCore] of credentialCores.entries()) {
            await stopCore(credentialCore, credentialConfigs[index]);
        }
        credentialCores.length = 0;

        const invalidMemberUpdate = await context.request.patch(`/api/v1/networks/${networkId}/members/${machineId}`, {
            data: { proxy_cidrs: ['2001:db8::/64'] },
        });
        assert.equal(invalidMemberUpdate.status(), 400, await invalidMemberUpdate.text());
        const memberUpdate = await context.request.patch(`/api/v1/networks/${networkId}/members/${machineId}`, {
            data: { proxy_cidrs: ['198.18.240.0/24'] },
        });
        assert.equal(memberUpdate.status(), 200, await memberUpdate.text());

        const { members } = await api(context, `/networks/${networkId}/members`);
        const memberId = members.find(member => member.device_id === machineId).member_id;
        const response = await context.request.put(`/api/v1/networks/${networkId}/acl-policy`, {
            data: {
                policy: {
                    default_action: 'allow',
                    rules: [{
                        id: '00000000-0000-0000-0000-000000000003',
                        name: 'DNS and ping',
                        enabled: true,
                        action: 'allow',
                        sources: [{ type: 'member', member_id: memberId }],
                        destinations: [{ type: 'subnet', member_id: memberId, cidrs: ['198.18.240.0/24'] }],
                        protocols: [
                            { protocol: 'udp', ports: ['53'], stateful: true },
                            { protocol: 'icmp', ports: [], stateful: false },
                        ],
                    }],
                },
            },
        });
        assert.equal(response.status(), 200, await response.text());

        const saved = await api(context, `/networks/${networkId}/acl-policy`);
        assert.equal(saved.policy.rules.length, 1);
        assert.equal(saved.policy.rules[0].sources[0].type, 'member');
        assert.deepEqual(saved.policy.rules[0].destinations[0], {
            type: 'subnet', member_id: saved.policy.rules[0].sources[0].member_id,
            cidrs: ['198.18.240.0/24'],
        });
        const deletion = await context.request.delete(`/api/v1/networks/${networkId}/members/${machineId}`);
        assert.equal(deletion.status(), 204, await deletion.text());
        const remaining = await api(context, `/networks/${networkId}/acl-policy`);
        assert.equal(remaining.policy.rules.length, 0);

        const addAgain = await context.request.post(`/api/v1/networks/${networkId}/members`, {
            data: { device_ids: [machineId] },
        });
        assert.equal(addAgain.status(), 204, await addAgain.text());
        await waitFor(async () => {
            const { members } = await api(context, `/networks/${networkId}/members`);
            return members.find(member => member.device_id === machineId && member.running);
        }, 'restored Core instance', 45000);
        assert.ok((await readdir(coreConfig)).includes(`${networkId}.toml`));

        await stopCore(core, coreConfig);
        core = undefined;
        const offlineDelete = await context.request.delete(`/api/v1/networks/${networkId}/members/${machineId}`);
        assert.equal(offlineDelete.status(), 204, await offlineDelete.text());
        await stopWeb(web);
        web = startWeb(database, apiPort, configPort, gatewayHost);
        await waitFor(async () => (await fetch(`${base}/api_meta.js`)).ok, 'restarted Web server');
        await login(page);
        core = startCore(coreConfig, gatewayHost, configPort);
        await waitFor(async () => {
            const { machines } = await api(context, '/machines');
            return machines.some(machine => machine.info.hostname === 'e2e-device' && machine.online);
        }, 'Core reconnect after Web restart');
        await waitFor(async () => {
            const state = await api(context, `/machines/${machineId}/networks`);
            return state.running_inst_ids?.length === 0;
        }, 'offline managed config removal', 45000);
        await waitFor(async () => !(await readdir(coreConfig)).includes(`${networkId}.toml`),
            'removed Core config file');
        assert.deepEqual((await api(context, `/networks/${networkId}/members`)).members, []);
    } catch (error) {
        throw new Error(`${error.message}\nWeb logs:\n${web.logs()}\nCore logs:\n${core?.logs() ?? ''}\nCredential Core logs:\n${credentialCores.map(item => item.logs()).join('\n')}`, { cause: error });
    } finally {
        await browser?.close();
        for (const [index, credentialCore] of credentialCores.entries()) {
            await stopCore(credentialCore, credentialConfigs[index]);
        }
        await stopCore(core, coreConfig);
        await stopWeb(web);
        await rm(temp, { recursive: true, force: true });
    }
});
