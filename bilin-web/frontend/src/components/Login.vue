<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { InputText, Password, Button } from 'primevue';
import { useRoute, useRouter } from 'vue-router';
import { useToast } from 'primevue/usetoast';
import { getApiBase } from "../modules/api-host"
import { useI18n } from 'vue-i18n'
import ApiClient, { type CentralNetworkSummary, type DeviceLoginDetails } from '../modules/api';
import DeviceLoginNetworkDialog from './DeviceLoginNetworkDialog.vue';

const { t } = useI18n()

const props = defineProps<{
    isRegistering: boolean;
    bindDevice?: boolean;
}>();

const api = new ApiClient(getApiBase());
const router = useRouter();
const route = useRoute();
const toast = useToast();

const username = ref('');
const password = ref('');
const registerUsername = ref('');
const registerPassword = ref('');
const captcha = ref('');
const captchaVersion = ref(0);
const captchaSrc = computed(() => api.captchaUrl() + '?v=' + captchaVersion.value);
const submitting = ref(false);

type BindingPhase = 'loading' | 'sign-in' | 'confirm' | 'complete' | 'expired' | 'unavailable';
const bindingPhase = ref<BindingPhase>('loading');
const bindingDetails = ref<DeviceLoginDetails>();
const bindingAccount = ref('');
const bindingError = ref('');
const networkDialogVisible = ref(false);
const joinedNetwork = ref<Pick<CentralNetworkSummary, 'network_id' | 'display_name'>>();
const bindingTicket = computed(() => typeof route.query.ticket === 'string' ? route.query.ticket : '');
const pageTitle = computed(() => {
    if (props.isRegistering) return '创建账号';
    if (!props.bindDevice) return '登录控制台';
    if (bindingPhase.value === 'complete') return '本机已绑定';
    return '登录并绑定本机';
});

function httpStatus(cause: unknown) {
    return (cause as { response?: { status: number } } | null)?.response?.status;
}

async function loadDeviceBinding() {
    const ticket = bindingTicket.value;
    bindingPhase.value = 'loading';
    bindingError.value = '';
    bindingDetails.value = undefined;
    networkDialogVisible.value = false;
    joinedNetwork.value = undefined;
    if (!/^[a-f0-9]{64}$/.test(ticket)) {
        bindingPhase.value = 'expired';
        return;
    }
    try {
        const details = await api.device_login_details(ticket);
        if (ticket !== bindingTicket.value) return;
        bindingDetails.value = details;
        try {
            const account = await api.get_console_info();
            if (ticket !== bindingTicket.value) return;
            bindingAccount.value = account.username;
            bindingPhase.value = 'confirm';
        } catch (cause) {
            if (httpStatus(cause) !== 401) throw cause;
            if (ticket !== bindingTicket.value) return;
            bindingPhase.value = 'sign-in';
        }
    } catch (cause) {
        if (ticket !== bindingTicket.value) return;
        const status = httpStatus(cause);
        bindingPhase.value = status === 404 || status === 410 ? 'expired' : 'unavailable';
    }
}

function switchBindingAccount() {
    bindingError.value = '';
    username.value = '';
    password.value = '';
    bindingPhase.value = 'sign-in';
}

async function approveDeviceBinding() {
    const ticket = bindingTicket.value;
    bindingError.value = '';
    try {
        await api.approve_device_login(ticket);
        if (ticket !== bindingTicket.value) return;
        bindingPhase.value = 'complete';
        networkDialogVisible.value = true;
        // Remove the consumed ticket from the address bar and browser history.
        await router.replace({ name: 'device-login' });
    } catch (cause) {
        if (ticket !== bindingTicket.value) return;
        switch (httpStatus(cause)) {
            case 401:
                switchBindingAccount();
                bindingError.value = '请登录后继续绑定。';
                break;
            case 404:
            case 410:
                bindingPhase.value = 'expired';
                break;
            case 403:
                bindingError.value = '当前账号无法绑定设备，请检查账号状态。';
                break;
            case 409:
                bindingPhase.value = 'expired';
                bindingError.value = '这台设备已有归属，请返回客户端查看。';
                break;
            default:
                bindingError.value = '绑定失败，请重试。';
        }
    }
}

async function confirmDeviceBinding() {
    if (submitting.value || bindingPhase.value !== 'confirm') return;
    submitting.value = true;
    try {
        await approveDeviceBinding();
    } finally {
        submitting.value = false;
    }
}

onMounted(() => { if (props.bindDevice) void loadDeviceBinding(); });
watch(bindingTicket, ticket => {
    if (props.bindDevice && (ticket || bindingPhase.value !== 'complete')) void loadDeviceBinding();
});

const onSubmit = async () => {
    if (submitting.value || (props.bindDevice && bindingPhase.value !== 'sign-in')) return;
    submitting.value = true;
    bindingError.value = '';
    try {
        const account = username.value.trim();
        const ticket = bindingTicket.value;
        await api.login(account, password.value);
        password.value = '';
        if (props.bindDevice) {
            if (ticket !== bindingTicket.value) return;
            bindingAccount.value = account;
            bindingPhase.value = 'confirm';
            await approveDeviceBinding();
            return;
        }
        const portal = route.query.portal;
        await router.replace({ name: portal === 'workspace' || portal === 'admin' || portal === 'admin-users' ? portal : 'home' });
    } catch (cause) {
        const status = (cause as { response?: { status: number } }).response?.status;
        toast.add({ severity: 'error', summary: status === 401 ? '用户名或密码错误' : '登录失败，请稍后重试', life: 4000 });
    } finally {
        submitting.value = false;
    }
};

const onRegister = async () => {
    if (submitting.value) return;
    submitting.value = true;
    try {
        await api.register(registerUsername.value.trim(), registerPassword.value, captcha.value.trim());
        toast.add({ severity: 'success', summary: '注册成功，请登录', life: 4000 });
        await router.replace({ name: 'login' });
    } catch (cause) {
        const status = (cause as { response?: { status: number } }).response?.status;
        const message = status === 403 ? '当前未开放注册' : '注册失败，请检查用户名和验证码';
        toast.add({ severity: 'error', summary: message, life: 4000 });
        captcha.value = '';
        captchaVersion.value++;
    } finally {
        submitting.value = false;
    }
};

</script>

<template>
    <main class="access-page">
        <div class="access-frame">
            <section class="access-brand dot-surface" aria-label="比邻组网控制台">
                <span class="console-mark"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                <div>
                    <h1>比邻组网</h1>
                    <p>网络管理控制台</p>
                </div>
            </section>

            <section class="access-panel" aria-labelledby="access-title">
                <div class="access-sheet">
                    <header><h2 id="access-title">{{ pageTitle }}</h2></header>
                    <div v-if="bindDevice && bindingDetails" class="binding-device">
                        <span class="work-icon"><i class="pi pi-desktop" aria-hidden="true" /></span>
                        <div><strong>{{ bindingDetails.hostname || '本机设备' }}</strong>
                            <small>设备编号 <code>{{ bindingDetails.device_code }}</code></small>
                        </div>
                    </div>
                    <form v-if="!isRegistering && (!bindDevice || bindingPhase === 'sign-in')" class="access-form" @submit.prevent="onSubmit">
                        <div class="access-field">
                            <label for="username">{{ t('web.login.username') }}</label>
                            <InputText id="username" v-model="username" autocomplete="username" required />
                        </div>
                        <div class="access-field">
                            <label for="password">{{ t('web.login.password') }}</label>
                            <Password input-id="password" v-model="password" autocomplete="current-password" required toggle-mask :feedback="false" />
                        </div>
                        <p v-if="bindingError" class="workspace-error" role="alert">{{ bindingError }}</p>
                        <Button :label="bindDevice ? '登录并绑定本机' : t('web.login.login')" icon="pi pi-arrow-right"
                            icon-pos="right" type="submit" class="access-submit"
                            :disabled="!username.trim() || !password" :loading="submitting" />
                    </form>

                    <form v-else-if="isRegistering" class="access-form" @submit.prevent="onRegister">
                        <div class="access-field">
                            <label for="register-username">{{ t('web.login.username') }}</label>
                            <InputText id="register-username" v-model="registerUsername" autocomplete="username" required />
                        </div>
                        <div class="access-field">
                            <label for="register-password">{{ t('web.login.password') }}</label>
                            <Password input-id="register-password" v-model="registerPassword" autocomplete="new-password" required toggle-mask :feedback="false" />
                        </div>
                        <div class="access-field">
                            <label for="captcha">{{ t('web.login.captcha') }}</label>
                            <div class="access-captcha">
                                <InputText id="captcha" v-model="captcha" required />
                                <button type="button" title="更换验证码" aria-label="更换验证码" @click="captchaVersion++">
                                    <img :src="captchaSrc" alt="验证码" />
                                </button>
                            </div>
                        </div>
                        <Button :label="t('web.login.register')" icon="pi pi-arrow-right" icon-pos="right" type="submit" class="access-submit"
                            :disabled="!registerUsername.trim() || !registerPassword || !captcha.trim()" :loading="submitting" />
                    </form>

                    <div v-else-if="bindingPhase === 'confirm'" class="access-form">
                        <p>将这台设备绑定到账号「{{ bindingAccount }}」。</p>
                        <p v-if="bindingError" class="workspace-error" role="alert">{{ bindingError }}</p>
                        <Button label="绑定本机" icon="pi pi-link" :loading="submitting" @click="confirmDeviceBinding" />
                        <Button label="切换账号" text :disabled="submitting" @click="switchBindingAccount" />
                    </div>
                    <div v-else-if="bindingPhase === 'complete'" class="access-form">
                        <p role="status">
                            已绑定到「{{ bindingAccount }}」。<template v-if="joinedNetwork">本机已加入「{{ joinedNetwork.display_name }}」。</template>
                        </p>
                        <RouterLink v-if="joinedNetwork" class="work-link"
                            :to="{ name: 'workspace', query: { view: 'networks', network: joinedNetwork.network_id } }">
                            查看网络 <i class="pi pi-arrow-up-right" aria-hidden="true" />
                        </RouterLink>
                        <Button v-else label="加入网络" outlined @click="networkDialogVisible = true" />
                        <DeviceLoginNetworkDialog v-if="bindingDetails" v-model:visible="networkDialogVisible"
                            :api="api" :account="bindingAccount" :device-code="bindingDetails.device_code"
                            @joined="joinedNetwork = $event" />
                    </div>
                    <p v-else-if="bindingPhase === 'expired'" class="workspace-error" role="alert">
                        {{ bindingError || '授权已失效，请回到客户端继续登录。' }}
                    </p>
                    <div v-else-if="bindingPhase === 'unavailable'" class="access-form">
                        <p class="workspace-error" role="alert">暂时无法读取设备授权。</p>
                        <Button outlined label="重试" @click="loadDeviceBinding" />
                    </div>
                    <p v-else role="status">正在读取设备授权…</p>

                    <footer v-if="!bindDevice" class="access-footer">
                        <RouterLink v-if="isRegistering" class="work-link" :to="{ name: 'login' }">
                            <i class="pi pi-arrow-left" aria-hidden="true" /> {{ t('web.login.back_to_login') }}
                        </RouterLink>
                        <RouterLink v-else class="work-link" :to="{ name: 'register' }">
                            {{ t('web.login.register') }} <i class="pi pi-arrow-up-right" aria-hidden="true" />
                        </RouterLink>
                    </footer>
                </div>
            </section>
        </div>
    </main>
</template>

<style scoped>
.access-page {
    display: grid;
    place-items: center;
    min-height: 100dvh;
    background: #eeefee;
    padding: 26px;
}
.access-frame {
    display: grid;
    grid-template-columns: minmax(0, 1.14fr) minmax(350px, 1fr);
    width: min(100%, 1050px);
    min-height: 610px;
    background: #fff;
    border: 1px solid #ddd;
    border-radius: 16px;
    overflow: hidden;
    box-shadow: 0 20px 70px #0000000c;
}
.access-brand {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: 28px;
    padding: clamp(25px, 5vw, 56px);
}
.access-brand .console-mark { width: 49px; height: 49px; font-size: 23px; }
.access-brand h1 {
    margin: 0 0 15px;
    font-size: clamp(45px, 7.5vw, 89px);
    font-weight: 700;
    letter-spacing: -.075em;
    line-height: 1.12;
}
.access-brand p { color: #bfbfbf; font-size: 15px; letter-spacing: .16em; margin: 0; }
.access-panel { display: grid; place-items: center; padding: clamp(25px, 5vw, 55px); }
.access-sheet { width: min(100%, 330px); }
.access-sheet header { margin-bottom: 23px; }
.access-sheet h2 { font-size: 26px; font-weight: 710; letter-spacing: -.035em; }
.access-form, .access-field { display: grid; gap: 14px; }
.access-field { gap: 6px; }
.access-field label { font-size: 13px; font-weight: 610; }
.binding-device {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 18px;
    padding: 11px 12px;
    border: 1px solid var(--console-line);
    border-radius: 9px;
    background: #f7f7f6;
}
.binding-device > div { min-width: 0; }
.binding-device strong { display: block; font-weight: 650; overflow-wrap: anywhere; }
.binding-device small { display: block; color: var(--console-dim); }
.access-field :deep(.p-inputtext) { min-height: 43px; width: 100%; }
.access-submit { width: 100%; min-height: 43px; margin-top: 5px; }
.access-footer { display: flex; justify-content: flex-end; margin-top: 20px; padding-top: 14px; border-top: 1px solid #ededed; }
.access-captcha { display: flex; gap: 7px; }
.access-captcha input { min-width: 0; flex: 1; }
.access-captcha button { width: 100px; flex: 0 0 auto; border: 1px solid #ddd; border-radius: 7px; overflow: hidden; }
.access-captcha img { width: 100%; height: 41px; object-fit: contain; filter: grayscale(1); }

@media (max-width: 680px) {
    .access-page { padding: 12px; align-items: start; }
    .access-frame { grid-template-columns: minmax(0, 1fr); min-height: auto; }
    .access-brand { padding: 19px; min-height: 152px; gap: 13px; }
    .access-brand .console-mark { width: 33px; height: 33px; font-size: 15px; border-radius: 8px; }
    .access-brand h1 { font-size: 34px; margin-bottom: 3px; }
    .access-brand p { font-size: 11px; }
    .access-panel { padding: 26px 18px; }
    .access-sheet header { margin-bottom: 17px; }
    .access-sheet h2 { font-size: 22px; }
}
</style>
