<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, shallowRef } from 'vue';
import { type LocationQuery, useRoute } from 'vue-router';
import DesktopAccount from '../../../../bilin-gui/src/components/DesktopAccount.vue';
import ApiClient, { type CentralNetworkSummary, type ControllerDevice } from '../modules/api';

const returnModes = {
    windows: 'http://tauri.localhost/',
    'windows-https': 'https://tauri.localhost/',
    macos: 'tauri://localhost/',
    dev: 'http://localhost:1420/',
} as const;

function readRequest(query: LocationQuery) {
    const { device, code, pairing, state, bound, setup, return: mode } = query;
    if (typeof device !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(device)) return;
    if (typeof state !== 'string' || !/^[0-9a-f]{32}$/.test(state)) return;
    if (typeof mode !== 'string' || !Object.prototype.hasOwnProperty.call(returnModes, mode)) return;
    if (code !== undefined && (typeof code !== 'string' || !/^[0-9a-z]{8}$/.test(code))) return;
    if (pairing !== undefined && (typeof pairing !== 'string' || !/^[0-9a-f]{16}$/.test(pairing))) return;
    if (bound !== undefined && bound !== '0' && bound !== '1') return;
    if (bound !== '1' && typeof pairing !== 'string') return;
    if (setup !== undefined && (setup !== '1' || bound !== '1')) return;
    return {
        device,
        state,
        pairing: typeof pairing === 'string' ? pairing : undefined,
        bound: bound === '1',
        setup: setup === '1',
        callback: returnModes[mode as keyof typeof returnModes],
    };
}

type Request = NonNullable<ReturnType<typeof readRequest>>;
type NetworkChoice = Pick<CentralNetworkSummary, 'network_id' | 'display_name'>;
type NetworkAccount = { active: Request; username: string; device: ControllerDevice };

const route = useRoute();
const request = computed(() => readRequest(route.query));
// The desktop window navigates to the controller itself so its session cookie
// and captcha stay on the origin that serves this page.
const api = new ApiClient('');
const busy = ref(false);
const error = ref('');
const captchaImage = ref('');
const captchaLoading = ref(false);
const checkingSession = ref(false);
const networkAccount = shallowRef<NetworkAccount>();
const networks = ref<NetworkChoice[]>([]);
const networkStep = ref<'loading' | 'joining' | 'choose' | 'create' | 'retry'>('loading');
const networkError = ref('');
const selectedNetwork = ref('');
const networkName = ref('');
const joining = computed(() => networkStep.value === 'loading' || networkStep.value === 'joining');
const invalidNetworkName = computed(() => !networkName.value.trim() || new TextEncoder().encode(networkName.value.trim()).length > 120);
let unmounted = false;

function isCurrent(active: Request) {
    return !unmounted && request.value === active;
}

function clearCaptcha() {
    if (captchaImage.value) URL.revokeObjectURL(captchaImage.value);
    captchaImage.value = '';
}

onUnmounted(() => {
    unmounted = true;
    clearCaptcha();
});

async function refreshCaptcha() {
    const active = request.value;
    if (!active || captchaLoading.value) return;
    captchaLoading.value = true;
    clearCaptcha();
    try {
        const response = await fetch(api.captchaUrl(), { credentials: 'same-origin', cache: 'no-store' });
        if (!response.ok || !response.headers.get('content-type')?.startsWith('image/')) throw new Error();
        const blob = await response.blob();
        if (isCurrent(active)) captchaImage.value = URL.createObjectURL(blob);
    } catch {
        if (isCurrent(active)) error.value = '验证码加载失败，请重试。';
    } finally {
        captchaLoading.value = false;
    }
}

function statusOf(cause: unknown) {
    return (cause as { response?: { status?: number } } | null)?.response?.status;
}

async function ownedDevice(active: Request) {
    const devices = await api.controller_devices();
    if (!Array.isArray(devices)) throw new Error();
    return devices.find(device => device.device_id === active.device && typeof device.owner_id === 'number');
}

function returnToDesktop(active: Request, username: string, networkId?: string) {
    if (!isCurrent(active)) return;
    const callback = new URL(active.callback);
    callback.searchParams.set('desktop-access-result', active.state);
    callback.searchParams.set('account', username);
    if (networkId) callback.searchParams.set('network', networkId);
    window.location.replace(callback.href);
}

function networkContextIsCurrent(context: NetworkAccount) {
    return networkAccount.value === context && isCurrent(context.active);
}

function handleNetworkError(context: NetworkAccount, cause: unknown, message: string) {
    if (!networkContextIsCurrent(context)) return;
    if (statusOf(cause) === 401) {
        networkAccount.value = undefined;
        error.value = '网页登录已失效，请重新登录。';
    } else {
        networkError.value = message;
    }
}

async function joinNetwork(context: NetworkAccount, networkId: string) {
    try {
        await api.add_controller_member(networkId, context.device.device_id);
    } catch (cause) {
        if (statusOf(cause) !== 409) throw cause;
        const members = await api.controller_members(networkId);
        if (!members.some(member => member.device_id === context.device.device_id)) throw cause;
    }
    if (networkContextIsCurrent(context)) returnToDesktop(context.active, context.username, networkId);
}

async function loadNetworks(context: NetworkAccount) {
    networkStep.value = 'loading';
    networkError.value = '';
    try {
        const listed = await api.list_networks();
        if (!networkContextIsCurrent(context)) return;
        const eligible = listed.filter(network => network.networking_method === 'Gateway' && network.secure_mode === true);
        if (eligible.some(network => context.device.assignments.some(assignment => assignment.network_id === network.network_id))) {
            returnToDesktop(context.active, context.username);
            return;
        }
        networks.value = eligible;
        selectedNetwork.value = eligible.length === 1 ? eligible[0].network_id : '';
        networkStep.value = eligible.length ? 'choose' : 'create';
    } catch (cause) {
        if (!networkContextIsCurrent(context)) return;
        networkStep.value = 'retry';
        handleNetworkError(context, cause, '无法读取可加入的网络，请重试。');
        return;
    }
    if (networks.value.length === 1) await submitNetwork();
}

async function beginNetworkSetup(active: Request, username: string, device: ControllerDevice) {
    if (!isCurrent(active)) return;
    const context = { active, username, device };
    networkAccount.value = context;
    await loadNetworks(context);
}

async function submitNetwork() {
    const context = networkAccount.value;
    if (!context || joining.value) return;
    const creating = networkStep.value === 'create';
    if (creating ? invalidNetworkName.value : !networks.value.some(network => network.network_id === selectedNetwork.value)) return;
    networkStep.value = 'joining';
    networkError.value = '';
    let created = false;
    try {
        const account = await api.get_console_info();
        if (!networkContextIsCurrent(context)) return;
        if (account.username !== context.username) {
            networkAccount.value = undefined;
            error.value = '请使用绑定本机时的账号加入网络。';
            return;
        }
        if (creating) {
            const display_name = networkName.value.trim();
            const { network_id } = await api.create_controller_network(display_name);
            if (!networkContextIsCurrent(context)) return;
            networks.value = [...networks.value, { network_id, display_name }];
            selectedNetwork.value = network_id;
            created = true;
        }
        await joinNetwork(context, selectedNetwork.value);
    } catch (cause) {
        if (!networkContextIsCurrent(context)) return;
        networkStep.value = creating && !created ? 'create' : 'choose';
        const message = created ? '网络已创建，暂时无法加入，请重试。' : creating
            ? '无法创建网络，请检查网络名称后重试。' : '无法加入网络，请重试。';
        handleNetworkError(context, cause, message);
    }
}

function continueWithoutNetwork() {
    const context = networkAccount.value;
    if (context && !joining.value) returnToDesktop(context.active, context.username);
}

onMounted(async () => {
    const active = request.value;
    if (!active?.setup) return;
    checkingSession.value = true;
    try {
        const account = await api.get_console_info();
        if (!isCurrent(active) || typeof account.username !== 'string' || !account.username) return;
        const device = await ownedDevice(active);
        if (device) await beginNetworkSetup(active, account.username, device);
    } catch {
        // A returned desktop login can outlive the browser session.
    } finally {
        checkingSession.value = false;
    }
});

async function authenticate(username: string, password: string, captcha?: string) {
    const active = request.value;
    if (!active || busy.value) return;
    busy.value = true;
    error.value = '';
    let step: 'register' | 'login' | 'binding' = captcha === undefined ? 'login' : 'register';
    try {
        if (captcha !== undefined) {
            await api.register(username, password, captcha);
            clearCaptcha();
        }
        if (!isCurrent(active)) return;
        step = 'login';
        await api.login(username, password);
        if (!isCurrent(active)) return;
        step = 'binding';
        const account = await api.get_console_info();
        if (!isCurrent(active)) return;
        if (account.username !== username) {
            error.value = '登录账号验证失败，请重新登录。';
            return;
        }
        let device = await ownedDevice(active);
        if (!device) {
            if (!isCurrent(active)) return;
            if (active.bound || !active.pairing) {
                error.value = '当前账号未绑定本机，请使用本机所属的账号登录。';
                return;
            }
            try {
                await api.claim_controller_device(active.device, active.pairing);
            } catch (cause) {
                // Ownership may have changed between the read and the claim.
                if (statusOf(cause) !== 409) throw cause;
            }
            if (!isCurrent(active)) return;
            device = await ownedDevice(active);
            if (!device) {
                if (isCurrent(active)) error.value = '本机已绑定其他账号，或绑定信息已更新。请使用本机所属的账号，或重新打开登录窗口。';
                return;
            }
        }
        await beginNetworkSetup(active, account.username, device);
    } catch (cause) {
        if (!isCurrent(active)) return;
        const status = statusOf(cause);
        if (step === 'register') {
            error.value = status === 403 ? '当前未开放注册。' : '注册失败，请检查用户名和验证码。';
            if (status !== 403) void refreshCaptcha();
        } else if (step === 'login') {
            error.value = status === 401 ? '用户名或密码错误。' : '登录失败，请稍后重试。';
        } else {
            error.value = '无法确认本机绑定状态，请重试。';
        }
    } finally {
        busy.value = false;
    }
}
</script>

<template>
    <main v-if="request && (checkingSession || networkAccount)" class="desktop-network-setup" aria-labelledby="desktop-network-setup-title">
        <section>
            <header>
                <i class="pi pi-sitemap" aria-hidden="true" />
                <h1 id="desktop-network-setup-title">{{ networkStep === 'create' ? '创建网络' : '加入网络' }}</h1>
            </header>
            <form @submit.prevent="networkAccount && (networkStep === 'retry' ? loadNetworks(networkAccount) : submitNetwork())">
                <p v-if="checkingSession || joining" role="status">
                    <i class="pi pi-spin pi-spinner" aria-hidden="true" />
                    {{ checkingSession ? '正在验证登录状态…' : networkStep === 'loading' ? '正在读取可加入的网络…' : '正在应用网络设置…' }}
                </p>
                <template v-else-if="networkStep === 'create'">
                    <p>账号还没有可加入的网络。创建一个网络即可加入。</p>
                    <label for="desktop-network-name">网络名称</label>
                    <input id="desktop-network-name" v-model="networkName" maxlength="120" placeholder="输入网络名称" required />
                </template>
                <template v-else-if="networkStep === 'choose'">
                    <label for="desktop-network-choice">选择要加入的网络</label>
                    <select id="desktop-network-choice" v-model="selectedNetwork" required>
                        <option value="" disabled>请选择网络</option>
                        <option v-for="network in networks" :key="network.network_id" :value="network.network_id">{{ network.display_name }}</option>
                    </select>
                </template>
                <p v-if="networkError" class="desktop-network-error" role="alert">{{ networkError }}</p>
                <footer v-if="!checkingSession && !joining">
                    <button type="button" @click="continueWithoutNetwork">稍后</button>
                    <button type="submit" :disabled="(networkStep === 'create' && invalidNetworkName) || (networkStep === 'choose' && !selectedNetwork)">
                        {{ networkStep === 'retry' ? '重试' : networkStep === 'create' ? '创建并加入' : '加入网络' }}
                    </button>
                </footer>
            </form>
        </section>
    </main>
    <DesktopAccount v-else-if="request" :busy="busy" :error="error" :captcha-image="captchaImage" :captcha-loading="captchaLoading"
        @sign-in="authenticate" @register="authenticate" @refresh-captcha="refreshCaptcha" @clear-error="error = ''" />
    <main v-else class="desktop-access-error" aria-labelledby="desktop-access-error-title">
        <div role="alert">
            <h1 id="desktop-access-error-title">登录连接无效</h1>
            <p>请返回比邻客户端重新打开登录窗口。</p>
        </div>
    </main>
</template>

<style scoped>
.desktop-network-setup {
    box-sizing: border-box;
    display: grid;
    justify-items: center;
    align-items: start;
    min-height: 100dvh;
    padding: 16px;
    color: #232623;
    background: #fcfdfa;
    font-family: system-ui, 'Microsoft YaHei', 'PingFang SC', sans-serif;
    font-size: 14px;
}

.desktop-network-setup section { width: min(100%, 420px); min-width: 0; }
.desktop-network-setup header {
    display: flex;
    position: relative;
    isolation: isolate;
    overflow: hidden;
    align-items: center;
    gap: 10px;
    min-height: 82px;
    padding: 16px;
    border-radius: 12px;
    color: #f9f9f9;
    background: #242623;
}
.desktop-network-setup header::before {
    position: absolute;
    z-index: -1;
    inset: 0;
    background: radial-gradient(circle, #626662 .95px, transparent 1.3px) 0 0 / 12px 12px;
    mask-image: linear-gradient(to right, transparent 18%, #000);
    opacity: .55;
    content: '';
}
.desktop-network-setup h1 { margin: 0; font-size: 22px; font-weight: 720; }
.desktop-network-setup form { display: grid; gap: 10px; padding: 16px 2px; }
.desktop-network-setup p { margin: 0 0 4px; }
.desktop-network-setup label { font-weight: 650; }
.desktop-network-setup :is(select, input) {
    box-sizing: border-box;
    width: 100%;
    min-width: 0;
    min-height: 44px;
    padding: 0 12px;
    border: 1px solid #d7ded6;
    border-radius: 9px;
    background: #fcfdfa;
    color: inherit;
    font: inherit;
}
.desktop-network-setup footer { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 8px; margin-top: 6px; }
.desktop-network-setup button { min-height: 38px; padding: 6px 12px; border-radius: 8px; font: inherit; }
.desktop-network-setup button[type='button'] { border: 1px solid #e0e4de; background: #eff2ee; }
.desktop-network-setup button[type='submit'] { color: #fff; background: #262925; }
.desktop-network-setup button:disabled { opacity: .55; cursor: not-allowed; }
.desktop-network-setup :is(input, select, button):focus-visible { outline: 2px solid #6a6a6a; outline-offset: 2px; }
.desktop-network-setup .desktop-network-error { color: #902a2a; }

.desktop-access-error {
    display: grid;
    min-height: 100dvh;
    place-items: center;
    padding: 16px;
    color: #222;
    background: #f4f4f4;
}
.desktop-access-error div {
    width: min(100%, 350px);
    padding: 24px;
    border: 1px solid #ddd;
    border-radius: 14px;
    background: #fff;
}
.desktop-access-error h1 { margin: 0 0 8px; font-size: 20px; font-weight: 700; }
.desktop-access-error p { margin: 0; font-size: 13px; }
</style>
