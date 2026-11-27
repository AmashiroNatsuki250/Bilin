<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { Button, Dialog, InputText, Select } from 'primevue'
import type ApiClient from '../modules/api'
import type { CentralNetworkSummary, ControllerDevice } from '../modules/api'
import type { GuideProgress, GuideStep } from '../modules/guide-progress'

type NetworkChoice = Pick<CentralNetworkSummary, 'network_id' | 'display_name'>

const props = defineProps<{
    api: ApiClient
    autoOpen: boolean
    devices: ControllerDevice[]
    networks: NetworkChoice[]
    initialProgress?: GuideProgress | null
    errorMessage?: (cause: unknown) => string
}>()
const emit = defineEmits<{
    changed: []
    progress: [value: GuideProgress | null]
    complete: [networkId: string]
}>()

const steps = [
    { title: '下载客户端', action: '我已安装' },
    { title: '查询设备编号', action: '我已找到编号' },
    { title: '绑定设备', action: '绑定设备' },
    { title: '选择网络', action: '继续' },
    { title: '加入网络', action: '加入网络' },
]

const visible = ref(false)
const pendingStep = ref<GuideStep>(props.initialProgress?.step ?? 0)
const step = ref<GuideStep>(props.initialProgress?.viewedStep ?? pendingStep.value)
const deviceCode = ref(props.initialProgress?.deviceCode ?? '')
const networkName = ref(props.initialProgress?.networkName ?? '')
const networkId = ref(props.initialProgress?.networkId ?? '')
const newNetworkName = ref('')
const selectedNetworkId = ref(networkId.value)
const createdNetwork = ref<NetworkChoice>()
const completed = ref(false)
const busy = ref(false)
const error = ref('')
const stepHeading = ref<HTMLElement | null>(null)
const current = computed(() => steps[step.value])
const normalizedCode = computed(() => deviceCode.value.trim().toLowerCase())
const networkOptions = computed(() => {
    const created = createdNetwork.value
    return created && !props.networks.some(network => network.network_id === created.network_id)
        ? [...props.networks, created]
        : props.networks
})
const selectedNetwork = computed(() => networkOptions.value.find(network => network.network_id === selectedNetworkId.value))
const targetNetwork = computed(() => networkOptions.value.find(network => network.network_id === networkId.value))
const actionLabel = computed(() => step.value === 3 && !networkOptions.value.length ? '创建网络' : current.value.action)
const canAdvance = computed(() => {
    if (step.value === 2) return /^[a-z0-9]{8}$/.test(normalizedCode.value)
    if (step.value === 3) {
        return networkOptions.value.length ? Boolean(selectedNetwork.value) : Boolean(newNetworkName.value.trim())
    }
    if (step.value === 4) return Boolean(targetNetwork.value && normalizedCode.value)
    return true
})

watch(() => props.networks, networks => {
    if (networks.some(network => network.network_id === createdNetwork.value?.network_id)) createdNetwork.value = undefined
})
watch(networkOptions, options => {
    if (!options.some(network => network.network_id === selectedNetworkId.value)) {
        selectedNetworkId.value = options.length === 1 ? options[0].network_id : ''
    }
}, { immediate: true })

onMounted(() => {
    visible.value = props.autoOpen
})

async function focusGuideStep() {
    await nextTick()
    if (visible.value) stepHeading.value?.focus()
}

watch([step, completed], focusGuideStep)

function openGuide() {
    visible.value = true
}

function finishGuide() {
    visible.value = false
    emit('complete', networkId.value)
}

function updateVisibility(show: boolean) {
    if (busy.value) return
    if (!show && completed.value) finishGuide()
    else visible.value = show
}

function previousStep() {
    error.value = ''
    step.value = (step.value - 1) as GuideStep
    saveProgress()
}

function advanceStep() {
    const next = (step.value + 1) as GuideProgress['step']
    step.value = next
    if (next > pendingStep.value) pendingStep.value = next
    saveProgress()
}

function saveProgress() {
    const furthestStep = pendingStep.value
    if (furthestStep === 0) return

    const progress: GuideProgress = { step: furthestStep, viewedStep: step.value }
    if (furthestStep >= 3) progress.deviceCode = normalizedCode.value
    if (furthestStep === 4) {
        progress.networkId = networkId.value
        progress.networkName = networkName.value
    }
    emit('progress', progress)
}

async function advance() {
    if (busy.value || !canAdvance.value || completed.value) return
    error.value = ''
    if (step.value < 2) {
        advanceStep()
        return
    }

    busy.value = true
    try {
        if (step.value === 2) {
            const code = normalizedCode.value
            if (!props.devices.some(device => device.device_code === code)) {
                try {
                    await props.api.claim_controller_device(code)
                } catch (cause) {
                    const owned = await props.api.controller_devices().catch(() => [])
                    if (!owned.some(device => device.device_code === code)) throw cause
                }
            }
            deviceCode.value = code
            emit('changed')
        } else if (step.value === 3) {
            let target = selectedNetwork.value
            if (!target) {
                const name = newNetworkName.value.trim()
                const created = await props.api.create_controller_network(name)
                target = { network_id: created.network_id, display_name: name }
                createdNetwork.value = target
                selectedNetworkId.value = target.network_id
                emit('changed')
            }
            networkId.value = target.network_id
            networkName.value = target.display_name
        } else {
            const id = networkId.value
            const code = normalizedCode.value
            try {
                await props.api.add_controller_member(id, code)
            } catch (cause) {
                const owned = await props.api.controller_devices().catch(() => [])
                if (!owned.some(device => device.device_code === code
                    && device.assignments.some(assignment => assignment.network_id === id))) throw cause
            }
            completed.value = true
            emit('progress', null)
            return
        }
        advanceStep()
    } catch (cause) {
        error.value = props.errorMessage?.(cause) || '操作失败，请重试。'
    } finally {
        busy.value = false
    }
}
</script>

<template>
    <section class="guide-sidebar dot-surface" aria-label="新手引导">
        <h2 class="guide-sidebar-title">
            <i class="pi pi-compass" aria-hidden="true" />
            新手引导
        </h2>
        <ol class="guide-sidebar-steps">
            <li v-for="(item, index) in steps" :key="item.title"
                :class="{ active: !completed && index === step, done: completed || index < pendingStep }"
                :aria-current="!completed && index === step ? 'step' : undefined">
                <button v-if="!completed && index === step" type="button" class="guide-sidebar-row"
                    aria-haspopup="dialog" :aria-expanded="visible" @click="openGuide">
                    <span class="guide-step-marker" aria-hidden="true">
                        <i v-if="index < pendingStep" class="pi pi-check" />
                        <template v-else>{{ index + 1 }}</template>
                    </span>
                    <span class="guide-sidebar-step-title">{{ item.title }}</span>
                    <i class="pi pi-chevron-right" aria-hidden="true" />
                </button>
                <span v-else class="guide-sidebar-row">
                    <span class="guide-step-marker" aria-hidden="true">
                        <i v-if="completed || index < pendingStep" class="pi pi-check" />
                        <template v-else>{{ index + 1 }}</template>
                    </span>
                    <span class="guide-sidebar-step-title">{{ item.title }}</span>
                </span>
                <span v-if="completed || index < pendingStep" class="sr-only">已完成</span>
            </li>
        </ol>
        <p class="guide-sidebar-tip">在客户端「设备与控制端」查看设备编号。</p>
    </section>

    <Dialog :visible="visible" modal class="guide-dialog" :draggable="false" :close-on-escape="!busy"
        :pt="{ root: { 'aria-labelledby': 'guide-dialog-title' } }"
        @show="focusGuideStep" @update:visible="updateVisibility">
        <template #container="{ closeCallback }">
            <div class="guide-dialog-layout">
                <aside class="guide-dialog-rail dot-surface">
                    <h2 id="guide-dialog-title">新手引导</h2>
                    <ol class="guide-dialog-steps" aria-label="组网步骤">
                        <li v-for="(item, index) in steps" :key="item.title"
                            :class="{ active: !completed && index === step, done: completed || index < pendingStep }"
                            :aria-current="!completed && index === step ? 'step' : undefined">
                            <span class="guide-dialog-number" aria-hidden="true">
                                <i v-if="completed || index < pendingStep" class="pi pi-check" />
                                <template v-else>{{ index + 1 }}</template>
                            </span>
                            <span class="guide-dialog-step-title">{{ item.title }}</span>
                            <span v-if="completed || index < pendingStep" class="sr-only">已完成</span>
                        </li>
                    </ol>
                </aside>

                <form class="guide-dialog-main" @submit.prevent="completed ? finishGuide() : advance()">
                    <header class="guide-dialog-heading">
                        <h3 ref="stepHeading" tabindex="-1">{{ completed ? '设备已加入网络' : current.title }}</h3>
                        <button type="button" class="guide-dialog-close" aria-label="关闭新手引导"
                            :disabled="busy" @click="closeCallback">
                            <i class="pi pi-times" aria-hidden="true" />
                        </button>
                    </header>
                    <div class="guide-dialog-content">
                        <div v-if="completed" class="guide-dialog-notice">
                            <span class="guide-dialog-icon"><i class="pi pi-check" aria-hidden="true" /></span>
                            <p>设备 <code>{{ deviceCode }}</code> 已加入「{{ networkName }}」。</p>
                        </div>
                        <div v-else-if="step === 0" class="guide-dialog-notice">
                            <span class="guide-dialog-icon"><i class="pi pi-download" aria-hidden="true" /></span>
                            <p>下载链接待补充。已安装客户端可直接继续。</p>
                        </div>
                        <div v-else-if="step === 1" class="guide-dialog-notice">
                            <span class="guide-dialog-icon"><i class="pi pi-id-card" aria-hidden="true" /></span>
                            <p>打开比邻客户端，进入「设备与控制端」。在「设备身份」里找到并复制八位设备编号。</p>
                        </div>
                        <div v-else-if="step === 2" class="guide-dialog-field">
                            <label for="guide-device-code">设备编号</label>
                            <InputText id="guide-device-code" v-model="deviceCode" placeholder="八位设备编号"
                                autocomplete="off" autocapitalize="none" :spellcheck="false" :disabled="busy"
                                @input="error = ''" />
                        </div>
                        <div v-else-if="step === 3" class="guide-dialog-field">
                            <template v-if="networkOptions.length">
                                <label for="guide-network-select">选择已有网络</label>
                                <Select v-model="selectedNetworkId" input-id="guide-network-select" aria-label="选择已有网络" :options="networkOptions"
                                    option-label="display_name" option-value="network_id" placeholder="选择要加入的网络"
                                    :disabled="busy" @update:model-value="error = ''" />
                            </template>
                            <template v-else>
                                <label for="guide-network-name">网络名称</label>
                                <InputText id="guide-network-name" v-model="newNetworkName" placeholder="输入网络名称"
                                    :disabled="busy" @input="error = ''" />
                            </template>
                        </div>
                        <dl v-else class="guide-join-summary">
                            <dt>设备编号</dt>
                            <dd><code>{{ deviceCode }}</code></dd>
                            <dt>网络名称</dt>
                            <dd>{{ targetNetwork?.display_name ?? networkName }}</dd>
                        </dl>
                        <p v-if="!completed && step === 4 && !targetNetwork" class="workspace-error" role="alert">
                            所选网络已不可用。返回上一步可重新选择。
                        </p>
                        <p v-if="error" class="workspace-error" role="alert">{{ error }}</p>
                    </div>
                    <footer class="guide-dialog-actions">
                        <Button v-if="!completed" type="button" label="稍后继续" severity="secondary"
                            text :disabled="busy" @click="closeCallback" />
                        <div class="guide-dialog-next">
                            <Button v-if="!completed && (step === 1 || step === 2 || step === 4)" type="button" label="上一步"
                                outlined :disabled="busy" @click="previousStep" />
                            <Button type="submit" :label="completed ? '完成' : actionLabel"
                                :icon="completed ? 'pi pi-check' : 'pi pi-arrow-right'" icon-pos="right"
                                :loading="busy" :disabled="!completed && !canAdvance" />
                        </div>
                    </footer>
                </form>
            </div>
        </template>
    </Dialog>
</template>

<style scoped>
.guide-sidebar {
    margin-top: auto;
    padding: 12px;
    border: 1px solid #414141;
    border-radius: 12px;
}

.guide-sidebar-title {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 3px 5px;
    font-size: 13px;
    font-weight: 660;
}

.guide-sidebar-steps {
    display: grid;
    gap: 2px;
    margin: 7px 0 0;
    color: #c6c6c6;
    font-size: 12px;
}

.guide-sidebar-row {
    display: flex;
    width: 100%;
    align-items: center;
    gap: 8px;
    padding: 4px 5px;
    border-radius: 6px;
    text-align: left;
}

.guide-sidebar-row > .pi-chevron-right {
    margin-inline-start: auto;
    color: #e0e0e0;
    font-size: 10px;
}

.guide-sidebar-steps .active { color: #fff; font-weight: 650; }
.guide-sidebar-steps .active > button { background: #373737; }
.guide-sidebar-steps .active > button:hover { background: #484848; }

.guide-step-marker {
    display: grid;
    width: 18px;
    height: 18px;
    flex: 0 0 18px;
    place-items: center;
    border: 1px solid #5e5e5e;
    border-radius: 50%;
    font-size: 10px;
}

.guide-step-marker > .pi {
    font-size: 8px;
}

.guide-sidebar-steps .active .guide-step-marker {
    border-color: #eee;
    background: #eee;
    color: #202020;
}

.guide-sidebar-steps .done:not(.active) .guide-step-marker {
    border-color: #737373;
    background: #404040;
    color: #fff;
}

.guide-sidebar-tip {
    margin: 10px 4px 0;
    color: #c5c5c5;
    font-size: 11px;
    line-height: 1.6;
}

:global(.p-dialog.guide-dialog) {
    width: min(736px, calc(100vw - 28px));
    max-height: calc(100dvh - 28px);
    overflow: auto;
    overscroll-behavior: contain;
    border: 1px solid #dededb;
    border-radius: 18px;
    box-shadow: 0 28px 80px #17171745;
}

.guide-dialog-layout {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    min-height: 376px;
}

.guide-dialog-rail {
    padding: 27px 19px;
}

.guide-dialog-rail h2 {
    margin: 0 8px 22px;
    font-size: 19px;
    font-weight: 670;
}

.guide-dialog-steps {
    display: grid;
    gap: 5px;
    font-size: 13px;
}

.guide-dialog-steps > li {
    display: flex;
    align-items: center;
    min-width: 0;
    gap: 10px;
    padding: 8px;
    border-radius: 8px;
    color: #bdbdbd;
}

.guide-dialog-steps > .active {
    background: #393939;
    color: #fff;
    font-weight: 650;
}

.guide-dialog-steps > .done { color: #efefef; }

.guide-dialog-number {
    display: grid;
    flex: 0 0 25px;
    width: 25px;
    height: 25px;
    place-items: center;
    border: 1px solid #686868;
    border-radius: 50%;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
}

.guide-dialog-number > .pi { font-size: 9px; }
.guide-dialog-steps .active .guide-dialog-number { border-color: #f6f6f6; background: #f6f6f6; color: #1b1b1b; }
.guide-dialog-steps .done:not(.active) .guide-dialog-number { background: #393939; }

.guide-dialog-main {
    display: flex;
    min-width: 0;
    flex-direction: column;
    background: #fff;
}

.guide-dialog-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 28px 24px 0 32px;
}

.guide-dialog-heading h3 {
    margin: 0;
    color: #1b1b1b;
    font-size: clamp(22px, 3.4vw, 29px);
    font-weight: 710;
    line-height: 1.3;
}

.guide-dialog-heading h3:focus { outline: none; }

.guide-dialog-close {
    display: grid;
    flex: 0 0 34px;
    width: 34px;
    height: 34px;
    place-items: center;
    border: 1px solid #e8e8e6;
    border-radius: 8px;
    color: #4e4e4e;
}

.guide-dialog-close:hover:enabled { background: #f0f0ee; }
.guide-dialog-close:disabled { opacity: .4; }

.guide-dialog-content {
    display: grid;
    flex: 1 0 auto;
    align-content: start;
    gap: 14px;
    min-width: 0;
    padding: 23px 32px;
}

.guide-dialog-notice,
.guide-join-summary {
    background: #f7f7f6;
    border: 1px solid #e8e8e6;
    border-radius: 11px;
}

.guide-dialog-notice {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    padding: 13px;
}

.guide-dialog-notice p {
    margin: 0;
    color: #363636;
    line-height: 1.65;
    overflow-wrap: anywhere;
}

.guide-dialog-icon {
    display: grid;
    flex: 0 0 33px;
    width: 33px;
    height: 33px;
    place-items: center;
    border-radius: 8px;
    background: #e9e9e7;
    color: #343434;
}

.guide-join-summary {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 11px 17px;
    margin: 0;
    padding: 14px 16px;
}

.guide-join-summary dt { color: var(--console-dim); }
.guide-join-summary dd { margin: 0; overflow-wrap: anywhere; }

.guide-dialog-field {
    display: grid;
    min-width: 0;
    gap: 8px;
}

.guide-dialog-field label { font-weight: 620; }
.guide-dialog-field input { width: 100%; min-width: 0; min-height: 44px; }

.guide-dialog-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 14px 24px;
    border-top: 1px solid #ececea;
}

.guide-dialog-next {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-inline-start: auto;
}


@media (max-height: 590px) {
    .guide-sidebar-steps {
        gap: 3px;
        margin-top: 4px;
    }
    .guide-sidebar-tip {
        display: none;
    }
}

@media (max-width: 760px) {
    .guide-sidebar {
        display: flex;
        align-self: flex-start;
        flex-wrap: wrap;
        align-items: center;
        gap: 2px 5px;
        margin: 4px 0 1px;
        padding: 3px 5px;
        border-radius: 7px;
    }

    .guide-sidebar-title { font-size: 12px; }
    .guide-sidebar-steps { display: flex; flex-wrap: wrap; align-items: center; margin: 0; }
    .guide-sidebar-row { width: auto; gap: 4px; padding-inline: 3px; }
    .guide-sidebar-tip { display: none; }

    .guide-sidebar-steps li:not(.active) .guide-sidebar-step-title,
    .guide-dialog-step-title {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
        white-space: nowrap;
    }

    .guide-dialog-layout { grid-template-columns: minmax(0, 1fr); min-height: 0; }
    .guide-dialog-main { min-height: 260px; }
    .guide-dialog-rail { padding: 16px 18px 12px; }
    .guide-dialog-rail h2 { margin: 0 0 9px; font-size: 15px; }
    .guide-dialog-steps { grid-template-columns: repeat(5, minmax(0, 1fr)); }
    .guide-dialog-steps > li { justify-content: center; padding: 4px; }
    .guide-dialog-heading { padding: 20px 17px 0; }
    .guide-dialog-content { padding: 14px 17px 17px; }
    .guide-dialog-actions { padding: 10px 12px; }
}
</style>
