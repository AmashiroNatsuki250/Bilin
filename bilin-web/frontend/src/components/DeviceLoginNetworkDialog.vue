<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Button, Dialog, InputText, Select } from 'primevue'
import type ApiClient from '../modules/api'
import type { CentralNetworkSummary } from '../modules/api'

type NetworkChoice = Pick<CentralNetworkSummary, 'network_id' | 'display_name'>

const props = defineProps<{
    api: ApiClient
    account: string
    deviceCode: string
    visible: boolean
}>()
const emit = defineEmits<{
    'update:visible': [visible: boolean]
    joined: [network: NetworkChoice]
}>()

const deviceId = ref('')
const networks = ref<NetworkChoice[]>([])
const selectedId = ref<string>()
const createName = ref('')
const creating = ref(false)
const loading = ref(false)
const submitting = ref(false)
const loadError = ref('')
const actionError = ref('')
const selectedNetwork = computed(() => networks.value.find(network => network.network_id === selectedId.value))
const canSubmit = computed(() => creating.value ? Boolean(createName.value.trim()) : Boolean(selectedNetwork.value))

function status(cause: unknown) {
    return (cause as { response?: { status: number } } | null)?.response?.status
}

async function loadNetworks() {
    if (loading.value) return
    loading.value = true
    loadError.value = ''
    try {
        const [account, devices, listed] = await Promise.all([
            props.api.get_console_info(),
            props.api.controller_devices(),
            props.api.list_networks(),
        ])
        if (account.username !== props.account) {
            loadError.value = '请使用绑定本机时的账号查看网络。'
            return
        }
        const device = devices.find(item => item.device_code === props.deviceCode)
        if (!device) {
            loadError.value = '当前账号找不到本机，请从客户端重新发起绑定。'
            return
        }
        deviceId.value = device.device_id
        networks.value = listed.filter(network => network.networking_method === 'Gateway' && network.secure_mode === true)
        selectedId.value = networks.value.length === 1 ? networks.value[0].network_id : undefined
        creating.value = networks.value.length === 0
    } catch (cause) {
        loadError.value = status(cause) === 401 ? '网页登录已失效，请从客户端重新打开登录页。' : '暂时无法读取网络，请重试。'
    } finally {
        loading.value = false
    }
}

watch(() => props.visible, visible => {
    if (visible && !deviceId.value) void loadNetworks()
}, { immediate: true })

function changeMode(create: boolean) {
    actionError.value = ''
    creating.value = create
}

async function submit() {
    if (submitting.value || !canSubmit.value || !deviceId.value) return
    submitting.value = true
    actionError.value = ''
    let created = false
    try {
        const account = await props.api.get_console_info()
        if (account.username !== props.account) {
            actionError.value = '请使用绑定本机时的账号加入网络。'
            return
        }
        let target = selectedNetwork.value
        if (creating.value) {
            const displayName = createName.value.trim()
            const result = await props.api.create_controller_network(displayName)
            target = { network_id: result.network_id, display_name: displayName }
            networks.value = [...networks.value, target]
            selectedId.value = target.network_id
            creating.value = false
            createName.value = ''
            created = true
        }
        if (!target) return
        const id = target.network_id
        try {
            await props.api.add_controller_member(id, deviceId.value)
        } catch (cause) {
            if (status(cause) !== 409) throw cause
            const members = await props.api.controller_members(id).catch(() => [])
            if (!members.some(member => member.device_id === deviceId.value)) throw cause
        }
        emit('joined', target)
        emit('update:visible', false)
    } catch (cause) {
        if (status(cause) === 401) actionError.value = '网页登录已失效，请从客户端重新打开登录页。'
        else if (created) actionError.value = '网络已创建。点击「加入网络」重试。'
        else if (creating.value) actionError.value = '无法创建网络，请检查网络名称后重试。'
        else actionError.value = '无法加入网络，请重试。'
    } finally {
        submitting.value = false
    }
}
</script>

<template>
    <Dialog :visible="visible" modal :draggable="false" :closable="!submitting" :close-on-escape="!submitting"
        header="加入网络" class="workspace-dialog" @update:visible="emit('update:visible', $event)">
        <form class="workspace-form" @submit.prevent="submit">
            <p v-if="loading">正在读取账号网络…</p>
            <p v-else-if="loadError" class="workspace-error" role="alert">{{ loadError }}</p>
            <template v-else-if="creating">
                <label for="bound-device-network-name">网络名称</label>
                <InputText id="bound-device-network-name" v-model="createName" :disabled="submitting"
                    placeholder="输入网络名称" />
            </template>
            <template v-else>
                <template v-if="networks.length > 1">
                    <label for="bound-device-network-select">选择已有网络</label>
                    <Select v-model="selectedId" input-id="bound-device-network-select" aria-label="选择已有网络"
                        :options="networks" option-label="display_name" option-value="network_id" placeholder="请选择网络"
                        :disabled="submitting" />
                </template>
                <p v-if="selectedNetwork">是否将本机加入「{{ selectedNetwork.display_name }}」？</p>
            </template>
            <p v-if="actionError" class="workspace-error" role="alert">{{ actionError }}</p>
            <div class="dialog-actions">
                <Button type="button" label="稍后加入" text :disabled="submitting" @click="emit('update:visible', false)" />
                <Button v-if="loadError" type="button" label="重试" outlined @click="loadNetworks" />
                <template v-else-if="!loading && deviceId">
                    <Button v-if="!creating" type="button" label="创建网络" outlined
                        :disabled="submitting" @click="changeMode(true)" />
                    <Button v-else-if="networks.length" type="button" label="返回选网" outlined
                        :disabled="submitting" @click="changeMode(false)" />
                    <Button type="submit" :label="creating ? '创建并加入' : '加入网络'"
                        :loading="submitting" :disabled="!canSubmit" />
                </template>
            </div>
        </form>
    </Dialog>
</template>
