<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { Button, Dialog, InputText, Password, Select } from 'primevue'
import ApiClient, {
    type CentralNetworkMember,
    type CentralNetworkSummary,
    type ControllerDevice,
    type ControllerOverview,
    type PlatformUser,
    type UserPolicyPatch,
} from '../modules/api'
import { getApiBase } from '../modules/api-host'
import { loadGuideProgress, writeGuideProgress, type GuideProgress } from '../modules/guide-progress'
import GettingStartedGuide from './GettingStartedGuide.vue'
import UserManagement from './UserManagement.vue'

const props = defineProps<{ admin?: boolean; home?: boolean; usersPage?: boolean }>()
const { t } = useI18n()
const router = useRouter()
const route = useRoute()
const api = new ApiClient(getApiBase(), () =>
    router.replace({ name: 'login', query: props.home ? {} : {
        portal: props.usersPage ? 'admin-users' : props.admin ? 'admin' : 'workspace',
    } }))

type WorkspaceNetwork = CentralNetworkSummary & { owner_id?: number }
type ConsoleSection = 'overview' | 'nodes' | 'devices' | 'networks' | 'users'
type ActionForm = 'bind' | 'network' | 'join-network' | 'node' | 'member' | 'password' | 'upstream-subnet' | 'rename-device'
const usersSection = { id: 'users', name: '用户管理', icon: 'pi pi-users' } as const
const sections: { id: ConsoleSection; name: string; icon: string }[] = [
    { id: 'overview', name: '组网概览', icon: 'pi pi-th-large' },
    { id: 'nodes', name: '节点服务器', icon: 'pi pi-server' },
    { id: 'devices', name: '设备管理', icon: 'pi pi-desktop' },
    { id: 'networks', name: '网络管理', icon: 'pi pi-sitemap' },
]
const visibleSections = computed(() => props.admin ? [...sections, usersSection] : sections.filter(item => item.id !== 'nodes'))
const section = computed(() => props.usersPage ? usersSection
    : visibleSections.value.find(item => item.id !== 'users' && item.id === route.query.view) ?? sections[0])
const role = computed(() => t(props.admin ? 'controller.admin_portal' : 'controller.user_portal'))
const formLabels = {
    bind: 'controller.bind_device',
    network: 'controller.create_network',
    'join-network': 'controller.add_device',
    node: 'controller.register_node',
    member: 'controller.add_device',
    password: 'web.settings.change_password',
    'upstream-subnet': 'controller.configure_upstream_subnet',
    'rename-device': 'controller.rename_device',
}
const form = ref<ActionForm | null>(null)
const formError = ref('')
const confirmation = ref<{ label: string; message: string; action: () => Promise<void>; destructive: boolean }>()
const confirmError = ref('')
const keyword = ref('')
const listStatus = ref<'all' | 'online' | 'offline'>('all')
const listPage = ref(1)
const pageSize = 10

function sectionLink(view: ConsoleSection, id?: string) {
    if (view === 'users') return { name: 'admin-users' }
    return {
        ...(props.usersPage ? { name: 'admin' } : {}),
        query: view === 'overview' ? {} : { view, ...(id ? { network: id } : {}) },
    }
}
const overview = ref<ControllerOverview | null>(null)
const consoleInfo = ref<Awaited<ReturnType<ApiClient['get_console_info']>> | null>(null)
const platformUsers = ref<PlatformUser[] | null>(null)
const devices = ref<ControllerDevice[]>([])
const networks = ref<WorkspaceNetwork[]>([])
const network = computed(() => section.value.id === 'networks'
    ? networks.value.find(item => item.network_id === route.query.network) ?? null : null)
const members = ref<CentralNetworkMember[]>([])
const memberRows = computed(() => {
    const current = network.value
    const devicesById = new Map(devices.value.map(device => [device.device_id, device]))
    return members.value.map(member => {
        const device = devicesById.get(member.device_id)
        const upstreamTarget = props.admin && current?.networking_method === 'Gateway'
            && device?.owner_id != null && device.owner_id === current.owner_id ? device : undefined
        const savedUpstream = upstreamTarget?.upstream_subnet_cidr
        const pendingUpstream = savedUpstream && !member.proxy_cidrs?.includes(savedUpstream) ? savedUpstream : undefined
        return { member, device, upstreamTarget, pendingUpstream }
    })
})
const membersLoading = ref(false)
const claimId = ref('')
const newNetwork = ref('')
const joinNetworkId = ref<string>()
const addId = ref('')
const memberMode = ref<'existing' | 'code'>('existing')
const memberCode = ref('')
const userWorkspaceLoaded = ref(false)
const guideAutoOpen = ref(false)
const guideInProgress = ref(false)
const guideProgress = ref<GuideProgress | null>(null)
const startWithDevices = ref(false)
const startCode = ref('')
const startDeviceCodes = ref<string[]>([])
const startAddedCodes = ref<string[]>([])
const startNetworkId = ref<string>()
const associateId = ref('')
const nodeName = ref('')
const nodeEndpoint = ref('')
const newNodeToken = ref('')
const owner = ref<number | undefined>()
const bindingOwner = ref<number | undefined>()
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const newPassword = ref('')
const upstreamDevice = ref<ControllerDevice>()
const upstreamSubnet = ref('')
const deviceToRename = ref<ControllerDevice>()
const deviceName = ref('')
const trimmedDeviceName = computed(() => deviceName.value.trim())
const userName = (id?: number | null) =>
    overview.value?.users.find(user => user.id === id)?.username || '—'
const deviceOptions = computed(() => devices.value
    .filter(device => props.admin ? device.owner_id === network.value?.owner_id
        : !members.value.some(member => member.device_id === device.device_id))
    .map(device => ({ label: `${device.hostname || '—'} · ${device.device_code}`, value: device.device_code })))
const useMemberCode = computed(() => !props.admin && (memberMode.value === 'code' || !deviceOptions.value.length))
const showGuide = computed(() => !props.admin && userWorkspaceLoaded.value
    && (guideAutoOpen.value || guideInProgress.value || !devices.value.length))
const deviceCodePattern = /^[a-z0-9]{8}$/i
const normalizedStartCode = computed(() => startCode.value.trim().toLowerCase())
const startPendingCode = computed(() => startAddedCodes.value.includes(normalizedStartCode.value) ? '' : normalizedStartCode.value)
const startPendingDevices = computed(() => devices.value.filter(device =>
    startDeviceCodes.value.includes(device.device_code) && !startAddedCodes.value.includes(device.device_code)))
const canStartNetwork = computed(() => Boolean(newNetwork.value.trim())
    && (!startWithDevices.value || (
        (!normalizedStartCode.value || deviceCodePattern.test(normalizedStartCode.value))
        && Boolean(startPendingDevices.value.length || startPendingCode.value)
    )))
const forwardingNodes = computed(() => overview.value?.forwarding_nodes ?? [])
const onlineDevices = computed(() => devices.value.filter(device => device.online).length)
const connectedDevices = computed(() => devices.value.filter(device => device.config_connected).length)
const readyNodes = computed(() => forwardingNodes.value.filter(node => node.online && node.ready).length)
const recentDevices = computed(() => [...devices.value].sort((a, b) => b.last_seen - a.last_seen).slice(0, 5))
const activeNetworks = computed(() => [...networks.value]
    .sort((a, b) => (b.online_member_count ?? 0) - (a.online_member_count ?? 0)).slice(0, 5))
const onlinePercent = computed(() => devices.value.length ? Math.round(onlineDevices.value / devices.value.length * 100) : 0)
const searches = { nodes: '搜索节点名称或地址', devices: '搜索主机名、设备编号或用户', networks: '搜索网络名称、网段或用户' }
const statuses = [{ id: 'all', label: '全部' }, { id: 'online', label: '在线' }, { id: 'offline', label: '离线' }] as const
const query = computed(() => keyword.value.trim().toLocaleLowerCase())
const matches = (...values: (string | null | undefined)[]) =>
    !query.value || values.some(value => value?.toLocaleLowerCase().includes(query.value))
const statusMatches = (online: boolean) => listStatus.value === 'all' || online === (listStatus.value === 'online')
const filteredNodes = computed(() => forwardingNodes.value.filter(node => statusMatches(node.online) && matches(node.name, node.peer_url)))
const filteredDevices = computed(() => devices.value.filter(device => statusMatches(device.online)
    && matches(device.hostname, device.device_code, props.admin ? userName(device.owner_id) : undefined)))
const filteredNetworks = computed(() => networks.value.filter(item => matches(item.display_name, item.virtual_cidr,
    props.admin ? userName(item.owner_id) : undefined)))
const listCount = computed(() => ({ nodes: filteredNodes.value.length, devices: filteredDevices.value.length,
    networks: filteredNetworks.value.length, overview: 0, users: 0 })[section.value.id])
const lastPage = computed(() => Math.max(1, Math.ceil(listCount.value / pageSize)))
const pageStart = computed(() => (listPage.value - 1) * pageSize)
const pageNodes = computed(() => filteredNodes.value.slice(pageStart.value, pageStart.value + pageSize))
const pageDevices = computed(() => filteredDevices.value.slice(pageStart.value, pageStart.value + pageSize))
const pageNetworks = computed(() => filteredNetworks.value.slice(pageStart.value, pageStart.value + pageSize))
watch([keyword, listStatus], () => { listPage.value = 1 })
watch(lastPage, page => { listPage.value = Math.min(listPage.value, page) })
watch(() => section.value.id, () => {
    keyword.value = ''
    listStatus.value = 'all'
    listPage.value = 1
})

function openForm(which: ActionForm) {
    formError.value = ''
    form.value = which
    if (which === 'bind') {
        associateId.value = ''
        bindingOwner.value = undefined
        claimId.value = ''
    } else if (which === 'network') {
        if (!startNetworkId.value) resetStartNetwork()
        owner.value = undefined
    } else if (which === 'join-network') {
        joinNetworkId.value = undefined
    } else if (which === 'node') {
        nodeName.value = ''
        nodeEndpoint.value = ''
        newNodeToken.value = ''
    } else if (which === 'member') {
        addId.value = ''
        memberCode.value = ''
        memberMode.value = deviceOptions.value.length ? 'existing' : 'code'
    } else if (which === 'password') {
        newPassword.value = ''
    }
}

function openUpstreamSubnet(device: ControllerDevice) {
    upstreamDevice.value = device
    upstreamSubnet.value = device.upstream_subnet_cidr ?? ''
    openForm('upstream-subnet')
}

function openDeviceRename(device: ControllerDevice) {
    deviceToRename.value = device
    deviceName.value = device.hostname
    openForm('rename-device')
}

function closeForm() {
    form.value = null
    newNodeToken.value = ''
    newPassword.value = ''
    upstreamDevice.value = undefined
    upstreamSubnet.value = ''
    deviceToRename.value = undefined
    deviceName.value = ''
}

function validUpstreamSubnet(cidr: string) {
    const match = /^(\d{1,3}(?:\.\d{1,3}){3})\/(1[6-9]|2\d|30)$/.exec(cidr)
    if (!match) return false
    const octets = match[1].split('.').map(Number)
    if (octets.some(octet => octet > 255) || octets.join('.') !== match[1]) return false
    const [first, second] = octets
    const privateAddress = first === 10 || (first === 172 && second >= 16 && second <= 31)
        || (first === 192 && second === 168)
    const address = octets.reduce((number, octet) => number * 256 + octet, 0)
    return privateAddress && address % 2 ** (32 - Number(match[2])) === 0
}

const formValid = computed(() => {
    switch (form.value) {
        case 'bind':
            return props.admin
                ? !!associateId.value.trim() && bindingOwner.value !== undefined
                : deviceCodePattern.test(claimId.value.trim())
        case 'network': return props.admin
            ? !!newNetwork.value.trim() && owner.value !== undefined
            : canStartNetwork.value
        case 'join-network': return networks.value.some(item => item.network_id === joinNetworkId.value)
        case 'node': return !!nodeName.value.trim() && !!nodeEndpoint.value.trim()
        case 'member': return useMemberCode.value
            ? deviceCodePattern.test(memberCode.value.trim())
            : !!addId.value.trim()
        case 'password': return !!newPassword.value
        case 'upstream-subnet':
            return !!props.admin && !!upstreamDevice.value
                && upstreamSubnet.value.trim() !== (upstreamDevice.value.upstream_subnet_cidr ?? '')
        case 'rename-device':
            return !!deviceToRename.value && !!trimmedDeviceName.value
                && trimmedDeviceName.value !== deviceToRename.value.hostname
        default: return false
    }
})

const pageAction = computed<ActionForm>(() => {
    if (network.value) return 'member'
    if (section.value.id === 'nodes') return 'node'
    if (section.value.id === 'devices') return 'bind'
    return 'network'
})
const createNetworkLabel = computed(() => startNetworkId.value ? '继续添加设备' : t(formLabels.network))
const pageActionLabel = computed(() => {
    if (!props.admin && pageAction.value === 'member') return '添加设备'
    if (!props.admin && pageAction.value === 'network') return createNetworkLabel.value
    return t(formLabels[pageAction.value])
})
const dialogTitle = computed(() => {
    if (form.value === 'member' && !props.admin) return '添加设备'
    if (form.value === 'join-network') return '加入已有网络'
    if (form.value === 'network' && !props.admin) return createNetworkLabel.value
    return form.value ? t(formLabels[form.value]) : ''
})
const formSubmitLabel = computed(() => {
    if (!form.value) return ''
    if (form.value === 'join-network') return '继续'
    if (form.value === 'network' && !props.admin) {
        if (startNetworkId.value) return '继续添加设备'
        if (startWithDevices.value) return '创建并加入'
    }
    if (form.value === 'password' || form.value === 'upstream-subnet' || form.value === 'rename-device') {
        return t('web.common.save')
    }
    return t(formLabels[form.value])
})

function errorMessage(cause: unknown, addingByCode = false) {
    const response = (cause as { response?: { status: number; data?: { message?: string } } }).response
    switch (response?.data?.message) {
        case 'change the default administrator password first': return '请先修改管理员初始密码'
        case 'invalid device number': return '请输入八位设备编号'
        case 'device is already bound or not registered': return '设备已绑定，或尚未登记'
        case 'device not found': return addingByCode
            ? '无法添加此设备，请核对编号和设备状态' : '找不到此设备，请核对设备编号'
        case 'confirm removal from all networks': return '设备已加入网络，请刷新后确认删除'
        case 'there is no forwarding node configured': return '请先注册并启动节点服务器'
        case 'invalid upstream subnet CIDR': return t('controller.invalid_upstream_subnet')
        case 'invalid device name': return t('controller.invalid_device_name')
        case 'bound device not found': return '设备已解绑或删除，请刷新后重试'
        case 'cannot ban your own account': return '无法封禁当前登录的管理员账号'
        case 'cannot delete your own account': return '无法删除当前登录的管理员账号'
        case 'cannot delete the built-in administrator account': return '无法删除内置管理员账号'
    }
    switch (response?.status) {
        case 401: return ''
        case 400: return '请检查填写的内容'
        case 403: return '当前账号无权执行此操作'
        case 404: return '找不到对应记录'
        case 409: return '记录状态已变更，请刷新后重试'
        default: return t('controller.operation_failed')
    }
}

function showError(cause: unknown) {
    error.value = errorMessage(cause)
}

let memberLoad: { networkId: string; ownerId?: number; task: Promise<void> } | undefined

async function loadMembers() {
    const current = network.value
    if (!current) return
    if (memberLoad?.networkId === current.network_id && memberLoad.ownerId === current.owner_id) return memberLoad.task
    membersLoading.value = true
    const task = api.controller_members(current.network_id, current.owner_id).then(result => {
        if (network.value?.network_id === current.network_id && network.value.owner_id === current.owner_id) {
            members.value = result
        }
    }).finally(() => {
        if (memberLoad?.task === task) {
            memberLoad = undefined
            membersLoading.value = false
        }
    })
    memberLoad = { networkId: current.network_id, ownerId: current.owner_id, task }
    return task
}

watch(() => route.query.network, () => {
    members.value = []
    if (network.value) void loadMembers().catch(showError)
})

let refreshInFlight: Promise<void> | undefined

function refresh() {
    return refreshInFlight ??= refreshWorkspace().finally(() => { refreshInFlight = undefined })
}

async function refreshWorkspace() {
    loading.value = true
    try {
        consoleInfo.value ??= await api.get_console_info()
        if (props.home || (props.admin && !consoleInfo.value.platform_admin)) {
            await router.replace({ name: props.home && consoleInfo.value.platform_admin ? 'admin' : 'workspace' })
            return
        }
        if (props.usersPage) {
            platformUsers.value = await api.platform_users()
        } else if (props.admin) {
            const result = await api.controller_overview()
            overview.value = result
            devices.value = result.devices
            networks.value = result.networks
        } else {
            const [deviceList, networkList] = await Promise.all([api.controller_devices(), api.list_networks()])
            devices.value = deviceList
            networks.value = networkList.filter(item => item.networking_method === 'Gateway')
            if (!userWorkspaceLoaded.value) {
                guideAutoOpen.value = !devices.value.length
                guideProgress.value = loadGuideProgress(consoleInfo.value.username, devices.value, networks.value)
                guideInProgress.value = guideProgress.value !== null
            }
            userWorkspaceLoaded.value = true
        }
        await loadMembers()
    } catch (cause) {
        showError(cause)
    } finally {
        loading.value = false
    }
}

async function refreshAfterChange() {
    await refreshInFlight
    await refresh()
}

async function mutate(action: () => Promise<void>, onError = showError) {
    if (saving.value) return
    error.value = ''
    saving.value = true
    try {
        await action()
        await refreshAfterChange()
    } catch (cause) {
        onError(cause)
    } finally {
        saving.value = false
    }
}

async function startNetwork() {
    const pendingDevices = startWithDevices.value ? startPendingDevices.value : []
    const code = startWithDevices.value ? startPendingCode.value : ''
    let completedId: string | undefined
    await mutate(async () => {
        startNetworkId.value ??= (await api.create_controller_network(newNetwork.value.trim())).network_id
        const id = startNetworkId.value
        if (code && !pendingDevices.some(device => device.device_code === code)) {
            await api.add_controller_member_by_code(id, code)
            startAddedCodes.value.push(code)
        }
        for (const device of pendingDevices) {
            try {
                await api.add_controller_member(id, device.device_code)
            } catch (cause) {
                const current = await api.controller_members(id).catch(() => [])
                if (!current.some(member => member.device_id === device.device_id)) throw cause
            }
            startAddedCodes.value.push(device.device_code)
        }
        completedId = id
    }, cause => {
        const message = errorMessage(cause, Boolean(code))
        formError.value = startNetworkId.value && message ? `网络已创建，设备添加未完成。${message}` : message
        if (startNetworkId.value) void refresh()
    })
    if (completedId) await openStartedNetwork(completedId)
}

function finishFirstRun() {
    guideAutoOpen.value = false
    guideInProgress.value = false
    saveGuideProgress(null)
}

function resetStartNetwork() {
    startNetworkId.value = undefined
    newNetwork.value = ''
    startWithDevices.value = false
    startCode.value = ''
    startDeviceCodes.value = []
    startAddedCodes.value = []
}

async function openStartedNetwork(id: string) {
    if (startAddedCodes.value.length) finishFirstRun()
    closeForm()
    resetStartNetwork()
    await router.push(sectionLink('networks', id))
}

async function openExistingNetwork(id: string) {
    await router.push(sectionLink('networks', id))
    if (network.value?.network_id !== id) return
    await loadMembers().catch(showError)
    openForm('member')
}

async function joinExistingNetwork() {
    if (networks.value.length === 1) await openExistingNetwork(networks.value[0].network_id)
    else if (networks.value.length > 1) openForm('join-network')
}

async function refreshGuide() {
    guideInProgress.value = true
    await refreshAfterChange()
}

function saveGuideProgress(progress: GuideProgress | null) {
    guideProgress.value = progress
    if (progress !== null) guideInProgress.value = true
    if (consoleInfo.value) writeGuideProgress(consoleInfo.value.username, progress)
}

async function openGuidedNetwork(id: string) {
    await refreshAfterChange()
    finishFirstRun()
    await router.push(sectionLink('networks', id))
}

async function submitForm() {
    if (!formValid.value || saving.value) return
    formError.value = ''
    if (form.value === 'network' && !props.admin) {
        await startNetwork()
        return
    }
    if (form.value === 'join-network' && joinNetworkId.value) {
        await openExistingNetwork(joinNetworkId.value)
        return
    }
    const subnet = upstreamSubnet.value.trim()
    if (form.value === 'upstream-subnet' && subnet && !validUpstreamSubnet(subnet)) {
        formError.value = t('controller.invalid_upstream_subnet')
        return
    }
    const name = trimmedDeviceName.value
    if (form.value === 'rename-device' && ([...name].length > 32 || /\p{Cc}/u.test(name))) {
        formError.value = t('controller.invalid_device_name')
        return
    }
    await mutate(async () => {
        switch (form.value) {
            case 'bind':
                if (props.admin) {
                    await api.bind_controller_device(associateId.value.trim(), bindingOwner.value!)
                } else {
                    await api.claim_controller_device(claimId.value.trim().toLowerCase())
                    guideAutoOpen.value = false
                }
                break
            case 'network':
                await api.create_controller_network(newNetwork.value.trim(), props.admin ? owner.value : undefined)
                break
            case 'node': {
                const result = await api.create_forwarding_node(nodeName.value.trim(), nodeEndpoint.value.trim())
                newNodeToken.value = result.registration_token
                return
            }
            case 'member': {
                const current = network.value
                if (!current) return
                if (useMemberCode.value) {
                    await api.add_controller_member_by_code(current.network_id, memberCode.value.trim().toLowerCase())
                } else {
                    await api.add_controller_member(current.network_id, addId.value.trim(), current.owner_id)
                }
                if (!props.admin) finishFirstRun()
                break
            }
            case 'password':
                await api.change_password(newPassword.value)
                await router.replace({ name: 'login' })
                break
            case 'upstream-subnet':
                await api.set_controller_device_upstream_subnet(upstreamDevice.value!.device_id, subnet || null)
                break
            case 'rename-device':
                await api.rename_controller_device(deviceToRename.value!.device_id, name, !!props.admin)
                break
        }
        closeForm()
    }, cause => { formError.value = errorMessage(cause, form.value === 'member' && useMemberCode.value) })
}

function ask(label: string, message: string, action: () => Promise<void>, destructive = false) {
    confirmError.value = ''
    confirmation.value = { label, message, action, destructive }
}

function confirmAction() {
    const task = confirmation.value
    if (!task) return
    void mutate(async () => {
        await task.action()
        confirmation.value = undefined
    }, cause => { confirmError.value = errorMessage(cause) })
}

function updateUserPolicy(user: PlatformUser, policy: UserPolicyPatch) {
    const save = async () => {
        const updated = await api.set_user_policy(user.id, policy)
        await refreshInFlight
        platformUsers.value = platformUsers.value?.map(item => item.id === updated.id ? updated : item) ?? null
    }
    if (policy.banned === true) {
        ask('封号', `封禁「${user.username}」后，该账号将无法登录，名下设备会断开连接。确认封禁吗？`, save)
    } else {
        void mutate(save)
    }
}

function deleteUser(user: PlatformUser) {
    ask('删除用户',
        `删除「${user.username}」后，该用户的登录会失效，账号和名下网络将被永久删除。名下设备会退出网络并解除绑定，可以重新绑定。`,
        () => api.delete_platform_user(user.id), true)
}

function unbindDevice(device: ControllerDevice) {
    ask(t('controller.unbind_device'), t('controller.confirm_unbind_device'),
        () => api.unbind_controller_device(device.device_id))
}

function deleteDevice(device: ControllerDevice) {
    const hasNetworks = device.assignments.length > 0
    const message = t(hasNetworks ? 'controller.confirm_delete_device_member' : 'controller.confirm_delete_device',
        { code: device.device_code })
    ask(t('controller.delete_device'), message,
        () => api.delete_controller_device(device.device_code, !!props.admin, hasNetworks))
}

function removeDevice(device: CentralNetworkMember) {
    const current = network.value
    if (!current) return
    ask(t('controller.remove_device'), t('controller.confirm_remove_device'),
        () => api.remove_controller_member(current.network_id, device.device_id, current.owner_id))
}

const copiedToken = ref(false)
async function copyToken() {
    try {
        await navigator.clipboard.writeText(newNodeToken.value)
        copiedToken.value = true
    } catch {
        formError.value = '复制失败，请选中注册码后手动复制。'
    }
}
watch(newNodeToken, () => { copiedToken.value = false })

function deleteNetwork() {
    const current = network.value
    if (!current) return
    ask(t('controller.delete_network'), t('controller.confirm_delete_network', { name: current.display_name }), async () => {
        await api.delete_controller_network(current.network_id, current.owner_id)
        if (current.network_id === startNetworkId.value) resetStartNetwork()
        await router.replace(sectionLink('networks'))
    })
}

async function logout() {
    try {
        await api.logout()
        await router.replace({ name: 'login' })
    } catch (cause) {
        showError(cause)
    }
}

const dateFormat = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short', hour12: false })
function when(timestamp?: number | null) {
    return timestamp ? dateFormat.format(new Date(timestamp * 1000)) : '—'
}

type ForwardingNode = ControllerOverview['forwarding_nodes'][number]
function nodeStatus(node: ForwardingNode) {
    return t(!node.online ? 'controller.offline' : node.ready ? 'controller.online' : 'controller.node_syncing')
}

function assignedNode(gateway: string | null) {
    if (!gateway) return t('controller.no_node')
    const node = overview.value?.forwarding_nodes.find(item => item.peer_url === gateway)
    return t('controller.assigned_node', { node: node?.name || gateway })
}

function memberStatus(member: CentralNetworkMember, device?: ControllerDevice) {
    if (member.running) return t('controller.network_running')
    if (member.error_msg) return t('controller.network_failed')
    if (device) {
        if (!device.online) return t('controller.offline')
        if (!device.config_connected) return t('controller.config_pending')
        if (!device.assignments.find(assignment => assignment.network_id === network.value?.network_id)?.gateway) {
            return '等待分配节点'
        }
    } else if (!member.online) {
        return t('controller.offline')
    }
    return t('controller.applying_assignment')
}

let timer: number | undefined
let nextRefresh = 0
onMounted(() => {
    void refresh()
    timer = window.setInterval(() => {
        if (saving.value) return
        const awaitingConfig = devices.value.some(device => device.online && device.owner_id && !device.config_connected)
        const awaitingNetwork = members.value.some(member => member.online && !member.running && !member.error_msg)
        if (Date.now() < nextRefresh && !awaitingConfig && !awaitingNetwork) return
        nextRefresh = Date.now() + 10_000
        void refresh()
    }, 2_000)
})
onUnmounted(() => window.clearInterval(timer))
</script>

<template>
    <main v-if="home || !consoleInfo || (admin && !consoleInfo.platform_admin)" class="console-wait" aria-live="polite">
        <i class="pi" :class="error ? 'pi-exclamation-circle' : 'pi-spin pi-spinner'" aria-hidden="true" />
        <p>{{ error || t('web.common.loading') }}</p>
        <Button v-if="error" label="重试" outlined @click="refresh" />
    </main>

    <div v-else class="workspace">
        <aside class="workspace-rail">
            <RouterLink :to="sectionLink('overview')" class="console-brand">
                <span class="console-mark"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                <span><strong>比邻</strong><small>组网控制台</small></span>
            </RouterLink>

            <nav class="workspace-nav" aria-label="控制台导航">
                <RouterLink v-for="item in visibleSections" :key="item.id" :to="sectionLink(item.id)"
                    :class="{ selected: section.id === item.id }" :aria-current="section.id === item.id ? 'page' : undefined"
                    active-class="" exact-active-class="">
                    <i :class="item.icon" aria-hidden="true" /><span>{{ item.name }}</span>
                </RouterLink>
            </nav>

            <GettingStartedGuide v-if="showGuide" :api="api" :auto-open="guideAutoOpen" :devices="devices" :networks="networks"
                :error-message="errorMessage" :initial-progress="guideProgress"
                @progress="saveGuideProgress" @changed="refreshGuide" @complete="openGuidedNetwork" />

            <div class="workspace-account" :class="{ 'with-guide': showGuide }">
                <div class="workspace-identity">
                    <span class="account-avatar" aria-hidden="true">{{ consoleInfo.username.slice(0, 1).toUpperCase() }}</span>
                    <span><strong>{{ consoleInfo.username }}</strong><small>{{ role }}</small></span>
                </div>
                <button type="button" :aria-label="t('web.settings.change_password')" :title="t('web.settings.change_password')"
                    @click="openForm('password')"><i class="pi pi-cog" aria-hidden="true" /></button>
                <button type="button" :aria-label="t('web.main.logout')" :title="t('web.main.logout')" @click="logout">
                    <i class="pi pi-sign-out" aria-hidden="true" />
                </button>
            </div>
        </aside>

        <main class="workspace-main">
            <header class="workspace-top">
                <nav class="workspace-breadcrumb" aria-label="当前位置">
                    <i class="pi pi-home" aria-hidden="true" />
                    <span>{{ role }}</span><i class="pi pi-chevron-right" aria-hidden="true" />
                    <RouterLink v-if="network" :to="sectionLink('networks')">{{ section.name }}</RouterLink>
                    <span v-else aria-current="page">{{ section.name }}</span>
                    <template v-if="network">
                        <i class="pi pi-chevron-right" aria-hidden="true" />
                        <span class="breadcrumb-network" aria-current="page">{{ network.display_name }}</span>
                    </template>
                </nav>
                <div class="workspace-top-actions">
                    <RouterLink v-if="consoleInfo.platform_admin" class="workspace-switch"
                        :to="{ name: admin ? 'workspace' : 'admin' }">
                        <i class="pi pi-arrow-right-arrow-left" aria-hidden="true" />
                        {{ t(admin ? 'controller.user_portal' : 'controller.admin_portal') }}
                    </RouterLink>
                    <button type="button" class="workspace-refresh" aria-label="刷新" title="刷新"
                        :disabled="loading || saving" @click="refresh">
                        <i class="pi pi-refresh" aria-hidden="true" />
                    </button>
                </div>
            </header>

            <div class="workspace-content" :class="{ 'user-network-content': !admin && section.id === 'networks' }">
                <header class="workspace-page-heading">
                    <div>
                        <RouterLink v-if="network" :to="sectionLink('networks')" class="work-link workspace-back">
                            <i class="pi pi-arrow-left" aria-hidden="true" /> 返回网络管理
                        </RouterLink>
                        <h1>{{ network?.display_name || section.name }}</h1>
                        <p v-if="network && admin">{{ t('controller.owner') }} · {{ userName(network.owner_id) }}</p>
                        <p v-else-if="usersPage">连接策略适用于账号下的所有设备。</p>
                    </div>
                    <div v-if="!usersPage && (admin || section.id !== 'overview')" class="workspace-heading-actions">
                        <Button v-if="network" :label="t('controller.delete_network')" icon="pi pi-trash" outlined
                            :disabled="saving" @click="deleteNetwork" />
                        <Button v-if="!network || network.networking_method === 'Gateway'" icon="pi pi-plus"
                            :label="pageActionLabel" :disabled="saving" @click="openForm(pageAction)" />
                    </div>
                </header>
                <p v-if="error" class="workspace-error" role="alert"><i class="pi pi-exclamation-circle" aria-hidden="true" /> {{ error }}</p>

                <UserManagement v-if="section.id === 'users'" :users="platformUsers" :current-user="consoleInfo.username"
                    :loading="loading" :saving="saving || Boolean(confirmation)"
                    @update-policy="updateUserPolicy" @delete-user="deleteUser" />

                <template v-else-if="section.id === 'overview'">
                    <div v-if="!admin && userWorkspaceLoaded" class="overview-shortcuts">
                        <section class="work-card overview-shortcut" aria-labelledby="overview-device-action">
                            <span class="work-icon"><i class="pi pi-desktop" aria-hidden="true" /></span>
                            <div class="shortcut-copy">
                                <h2 id="overview-device-action">设备</h2>
                                <p>用八位编号把设备绑定到账号。</p>
                            </div>
                            <div class="shortcut-actions">
                                <Button type="button" label="绑定设备" icon="pi pi-link" :disabled="saving" @click="openForm('bind')" />
                            </div>
                        </section>
                        <section class="work-card overview-shortcut" aria-labelledby="overview-network-action">
                            <span class="work-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                            <div class="shortcut-copy">
                                <h2 id="overview-network-action">网络</h2>
                                <p>{{ networks.length ? '把设备加入已有网络，或按需创建新网络。' : '创建网络后，可随时添加设备。' }}</p>
                            </div>
                            <div class="shortcut-actions">
                                <Button v-if="networks.length" type="button" label="加入已有网络" icon="pi pi-sign-in"
                                    :disabled="saving" @click="joinExistingNetwork" />
                                <Button type="button" :label="createNetworkLabel" icon="pi pi-plus" :outlined="Boolean(networks.length)"
                                    :disabled="saving" @click="openForm('network')" />
                            </div>
                        </section>
                    </div>

                    <div v-if="admin || userWorkspaceLoaded" class="overview-top">
                        <section class="work-hero dot-surface" aria-labelledby="overview-device-title">
                            <header><h2 id="overview-device-title"><i class="pi pi-desktop" aria-hidden="true" /> 在线设备</h2>
                                <RouterLink :to="sectionLink('devices')" class="hero-link">设备管理 <i class="pi pi-arrow-up-right" aria-hidden="true" /></RouterLink>
                            </header>
                            <div class="hero-number">{{ onlineDevices }} <span>/ {{ devices.length }}</span></div>
                            <div class="hero-progress" role="progressbar" aria-label="设备在线比例" :aria-valuenow="onlinePercent">
                                <span :style="{ width: onlinePercent + '%' }" />
                            </div>
                            <dl class="hero-facts">
                                <div><dt>配置通道已连接</dt><dd>{{ connectedDevices }}</dd></div>
                                <div><dt>离线设备</dt><dd>{{ devices.length - onlineDevices }}</dd></div>
                            </dl>
                        </section>

                        <div class="overview-metrics">
                            <RouterLink v-if="admin" :to="sectionLink('nodes')" class="metric-card">
                                <span class="work-icon"><i class="pi pi-server" aria-hidden="true" /></span>
                                <span><strong>节点服务器</strong><small>当前可用</small></span>
                                <strong class="metric-number">{{ readyNodes }}<small> / {{ forwardingNodes.length }}</small></strong>
                                <i class="pi pi-chevron-right" aria-hidden="true" />
                            </RouterLink>
                            <RouterLink :to="sectionLink('networks')" class="metric-card">
                                <span class="work-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                                <span><strong>{{ admin ? '网络总数' : '我的网络' }}</strong></span>
                                <strong class="metric-number">{{ networks.length }}</strong>
                                <i class="pi pi-chevron-right" aria-hidden="true" />
                            </RouterLink>
                            <RouterLink v-if="!admin" :to="sectionLink('devices')" class="metric-card">
                                <span class="work-icon"><i class="pi pi-id-card" aria-hidden="true" /></span>
                                <span><strong>我的设备</strong></span>
                                <strong class="metric-number">{{ devices.length }}</strong>
                                <i class="pi pi-chevron-right" aria-hidden="true" />
                            </RouterLink>
                        </div>
                    </div>

                    <div v-if="admin || userWorkspaceLoaded" class="overview-lists">
                        <section class="work-card overview-list">
                            <header class="work-card-heading"><h2>最近在线设备</h2>
                                <RouterLink class="work-link" :to="sectionLink('devices')">查看设备 <i class="pi pi-arrow-right" aria-hidden="true" /></RouterLink>
                            </header>
                            <ul v-if="recentDevices.length" class="snapshot-list">
                                <li v-for="device in recentDevices" :key="device.device_id">
                                    <span class="work-icon"><i class="pi pi-desktop" aria-hidden="true" /></span>
                                    <span class="snapshot-name"><strong>{{ device.hostname || '未命名设备' }}</strong><code>{{ device.device_code }}</code></span>
                                    <span class="work-chip" :class="{ 'chip-online': device.online }">{{ t(device.online ? 'controller.registration_online' : 'controller.registration_offline') }}</span>
                                </li>
                            </ul>
                            <p v-else class="snapshot-empty">{{ t('controller.no_devices') }}</p>
                        </section>
                        <section class="work-card overview-list">
                            <header class="work-card-heading"><h2>网络运行情况</h2>
                                <RouterLink class="work-link" :to="sectionLink('networks')">查看网络 <i class="pi pi-arrow-right" aria-hidden="true" /></RouterLink>
                            </header>
                            <ul v-if="activeNetworks.length" class="snapshot-list">
                                <li v-for="item in activeNetworks" :key="item.network_id">
                                    <RouterLink class="snapshot-network" :to="sectionLink('networks', item.network_id)">
                                        <span class="work-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                                        <span class="snapshot-name"><strong>{{ item.display_name }}</strong><code>{{ item.virtual_cidr || '—' }}</code></span>
                                        <span class="snapshot-count">{{ item.online_member_count ?? 0 }} / {{ item.member_count ?? 0 }} 在线</span>
                                        <i class="pi pi-chevron-right" aria-hidden="true" />
                                    </RouterLink>
                                </li>
                            </ul>
                            <p v-else class="snapshot-empty">{{ t('controller.no_network') }}</p>
                        </section>
                    </div>
                </template>

                <section v-else-if="!admin && section.id === 'networks' && !network" class="user-network-catalog" aria-label="网络列表">
                    <div class="user-network-toolbar">
                        <label class="resource-search">
                            <i class="pi pi-search" aria-hidden="true" />
                            <input v-model="keyword" type="search" placeholder="搜索网络名称或网段" aria-label="搜索网络名称或网段" />
                        </label>
                        <p v-if="listCount || !loading" role="status">{{ listCount }} 个网络</p>
                    </div>

                    <ul v-if="pageNetworks.length" class="user-network-grid">
                        <li v-for="item in pageNetworks" :key="item.network_id">
                            <RouterLink :to="sectionLink('networks', item.network_id)" class="user-network-card">
                                <div class="user-network-card-heading dot-surface">
                                    <span class="work-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                                    <strong>{{ item.display_name }}</strong>
                                    <i class="pi pi-arrow-right" aria-hidden="true" />
                                </div>
                                <dl class="user-network-card-facts">
                                    <div><dt>组网网段</dt><dd><code>{{ item.virtual_cidr || '—' }}</code></dd></div>
                                    <div><dt>在线成员</dt><dd class="user-network-online" :class="{ online: (item.online_member_count ?? 0) > 0 }">
                                        <strong>{{ item.online_member_count ?? 0 }}</strong><span> / {{ item.member_count ?? 0 }}</span>
                                    </dd></div>
                                </dl>
                            </RouterLink>
                        </li>
                    </ul>
                    <div v-else-if="!loading || keyword.trim()" class="work-card work-empty user-network-empty">
                        <span class="work-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                        <p>{{ keyword.trim() ? '没有匹配的记录' : t('controller.no_network') }}</p>
                    </div>

                    <nav v-if="lastPage > 1" class="resource-pagination user-network-pagination" aria-label="网络列表分页">
                        <button type="button" aria-label="上一页" :disabled="listPage === 1" @click="listPage--"><i class="pi pi-chevron-left" aria-hidden="true" /></button>
                        <span>第 {{ listPage }} / {{ lastPage }} 页</span>
                        <button type="button" aria-label="下一页" :disabled="listPage === lastPage" @click="listPage++"><i class="pi pi-chevron-right" aria-hidden="true" /></button>
                    </nav>
                </section>

                <template v-else-if="!network">
                    <section class="work-card resource-list" :aria-label="section.name">
                        <div class="resource-toolbar">
                            <div v-if="section.id !== 'networks'" class="resource-filters" aria-label="按在线状态过滤">
                                <button v-for="item in statuses" :key="item.id" type="button"
                                    :aria-pressed="listStatus === item.id" @click="listStatus = item.id">{{ item.label }}</button>
                            </div>
                            <label class="resource-search">
                                <i class="pi pi-search" aria-hidden="true" />
                                <input v-model="keyword" type="search" :placeholder="searches[section.id]" :aria-label="searches[section.id]" />
                            </label>
                        </div>

                        <div v-if="section.id === 'nodes' && overview?.gateway" class="gateway-line">
                            <i class="pi pi-server" aria-hidden="true" /><code>{{ overview.gateway }}</code>
                            <span class="work-chip" :class="{ 'chip-online': overview.gateway_forwarding }">
                                {{ t(overview.gateway_forwarding ? 'controller.forwarding_enabled' : 'controller.forwarding_disabled') }}
                            </span>
                        </div>

                        <div v-if="section.id === 'nodes' && pageNodes.length" class="work-table-scroll" tabindex="0" role="region" aria-label="节点服务器列表">
                            <table class="work-table node-table">
                                <thead><tr><th scope="col">节点名称</th><th scope="col">节点公开地址</th><th scope="col">状态</th><th scope="col">最后在线</th></tr></thead>
                                <tbody><tr v-for="node in pageNodes" :key="node.id">
                                    <th scope="row"><span class="table-entity"><span class="work-icon"><i class="pi pi-server" aria-hidden="true" /></span>{{ node.name }}</span></th>
                                    <td><code>{{ node.peer_url }}</code></td>
                                    <td><span class="work-chip" :class="{ 'chip-online': node.online && node.ready }">{{ nodeStatus(node) }}</span></td>
                                    <td>{{ when(node.last_seen) }}</td>
                                </tr></tbody>
                            </table>
                        </div>

                        <div v-if="section.id === 'devices' && pageDevices.length" class="work-table-scroll" tabindex="0" role="region" aria-label="设备列表">
                            <table class="work-table device-table">
                                <thead><tr><th scope="col">设备</th><th v-if="admin" scope="col">归属用户</th>
                                    <th scope="col">状态</th><th scope="col">最后在线</th><th scope="col">网络和分配节点</th><th scope="col">操作</th></tr></thead>
                                <tbody><tr v-for="device in pageDevices" :key="device.device_id">
                                    <th scope="row"><span class="table-entity"><span class="work-icon"><i class="pi pi-desktop" aria-hidden="true" /></span>
                                        <span><strong>{{ device.hostname || '未命名设备' }}</strong><code>{{ device.device_code }}</code></span>
                                    </span></th>
                                    <td v-if="admin">{{ userName(device.owner_id) }}</td>
                                    <td><span class="work-chip" :class="{ 'chip-online': device.online }">{{ t(device.online ? 'controller.registration_online' : 'controller.registration_offline') }}</span>
                                        <small v-if="device.online || device.config_connected">{{ t(device.config_connected
                                            ? 'controller.config_connected' : device.owner_id ? 'controller.config_pending' : 'controller.waiting_for_owner') }}</small></td>
                                    <td>{{ when(device.last_seen) }}<small v-if="device.version">{{ device.version }}</small></td>
                                    <td><div v-for="assignment in device.assignments" :key="assignment.network_id" class="device-assignment">
                                        <strong>{{ assignment.display_name }}</strong>
                                        <small :title="assignment.gateway ?? undefined">{{ assignedNode(assignment.gateway) }}</small>
                                        <small v-if="assignment.path && assignment.path !== 'single_device' && assignment.path !== 'unobserved'">
                                            {{ t('controller.device_path', { path: t('controller.path_' + assignment.path) }) }}
                                        </small>
                                    </div><span v-if="!device.assignments.length" class="work-muted">{{ t('controller.no_network') }}</span></td>
                                    <td><div class="row-actions">
                                        <button type="button" :disabled="saving" @click="openDeviceRename(device)">{{ t('controller.rename_device') }}</button>
                                        <button v-if="admin && device.owner_id !== null" type="button" :disabled="saving" @click="openUpstreamSubnet(device)">{{ t('controller.configure_upstream_subnet') }}</button>
                                        <button v-if="!admin" type="button" :disabled="saving" @click="unbindDevice(device)">{{ t('controller.unbind_device') }}</button>
                                        <button type="button" :disabled="saving" @click="deleteDevice(device)">{{ t('controller.delete_device') }}</button>
                                    </div>
                                    <small v-if="admin && device.upstream_subnet_cidr">{{ t('controller.upstream_subnet') }} · <code>{{ device.upstream_subnet_cidr }}</code></small></td>
                                </tr></tbody>
                            </table>
                        </div>

                        <ul v-if="section.id === 'networks' && pageNetworks.length" class="network-catalog">
                            <li v-for="item in pageNetworks" :key="item.network_id">
                                <RouterLink :to="sectionLink('networks', item.network_id)" class="network-tile">
                                    <span class="work-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
                                    <div class="network-tile-name"><strong>{{ item.display_name }}</strong><small v-if="admin">{{ userName(item.owner_id) }}</small></div>
                                    <code>{{ item.virtual_cidr || '—' }}</code>
                                    <span class="work-chip" :class="{ 'chip-online': item.online_member_count > 0 }">{{ item.online_member_count ?? 0 }} / {{ item.member_count ?? 0 }} 在线</span>
                                    <i class="pi pi-chevron-right" aria-hidden="true" />
                                </RouterLink>
                            </li>
                        </ul>

                        <div v-if="!listCount && !loading" class="work-empty">
                            <span class="work-icon"><i :class="section.icon" aria-hidden="true" /></span>
                            <p>{{ keyword.trim() || listStatus !== 'all' ? '没有匹配的记录' : t(section.id === 'nodes' ? 'controller.no_node' : section.id === 'devices' ? 'controller.no_devices' : 'controller.no_network') }}</p>
                        </div>
                        <footer v-if="listCount" class="resource-footer">
                            <span>共 {{ listCount }} 项</span>
                            <div v-if="lastPage > 1" class="resource-pagination">
                                <button type="button" aria-label="上一页" :disabled="listPage === 1" @click="listPage--"><i class="pi pi-chevron-left" aria-hidden="true" /></button>
                                <span>第 {{ listPage }} / {{ lastPage }} 页</span>
                                <button type="button" aria-label="下一页" :disabled="listPage === lastPage" @click="listPage++"><i class="pi pi-chevron-right" aria-hidden="true" /></button>
                            </div>
                        </footer>
                    </section>
                </template>

                <template v-else>
                    <section class="work-hero detail-hero dot-surface" aria-label="网络状态">
                        <dl class="hero-facts">
                            <div><dt>组网网段</dt><dd><code>{{ network.virtual_cidr || '—' }}</code></dd></div>
                            <div><dt>网络成员</dt><dd>{{ network.member_count ?? members.length }}</dd></div>
                            <div><dt>在线成员</dt><dd>{{ network.online_member_count ?? members.filter(member => member.online).length }}</dd></div>
                        </dl>
                    </section>
                    <section class="work-card member-panel">
                        <header class="work-card-heading"><h2>{{ t('controller.members') }}</h2></header>
                        <div v-if="members.length" class="work-table-scroll" tabindex="0" role="region" aria-label="网络成员列表">
                            <table class="work-table member-table">
                                <thead><tr><th scope="col">成员设备</th><th scope="col">状态</th><th scope="col">虚拟地址</th>
                                    <th scope="col">子网代理网段</th><th scope="col">操作</th></tr></thead>
                                <tbody><tr v-for="{ member, device, upstreamTarget, pendingUpstream } in memberRows" :key="member.device_id">
                                    <th scope="row"><span class="table-entity"><span class="work-icon"><i class="pi pi-desktop" aria-hidden="true" /></span>
                                        <span><strong>{{ member.hostname || '未命名设备' }}</strong><code>{{ device?.device_code || '—' }}</code></span>
                                    </span></th>
                                    <td><span class="work-chip" :class="{ 'chip-online': member.running && !member.error_msg }">{{ memberStatus(member, device) }}</span></td>
                                    <td><code>{{ member.runtime_virtual_ipv4 || member.allocated_ipv4 || member.virtual_ipv4 || '—' }}</code></td>
                                    <td>
                                        <code v-for="cidr in member.proxy_cidrs" :key="cidr" class="member-subnet">{{ cidr }}</code>
                                        <small v-if="pendingUpstream">{{ t('controller.upstream_subnet_saved') }} · <code>{{ pendingUpstream }}</code> · {{ t('controller.upstream_subnet_pending') }}</small>
                                        <span v-else-if="!member.proxy_cidrs?.length" class="work-muted">—</span>
                                    </td>
                                    <td><div class="row-actions">
                                        <button v-if="upstreamTarget" type="button" :disabled="saving" @click="openUpstreamSubnet(upstreamTarget)">{{ t('controller.configure_upstream_subnet') }}</button>
                                        <button type="button" :disabled="saving" @click="removeDevice(member)">{{ t('controller.remove_device') }}</button>
                                    </div></td>
                                </tr></tbody>
                            </table>
                        </div>
                        <div v-else class="work-empty"><span class="work-icon"><i class="pi pi-users" aria-hidden="true" /></span>
                            <p>{{ t(loading || membersLoading ? 'web.common.loading' : 'controller.no_members') }}</p></div>
                    </section>
                </template>
            </div>
        </main>

        <Dialog :visible="form !== null" modal :header="dialogTitle" class="workspace-dialog"
            :closable="!saving" :close-on-escape="!saving" @update:visible="visible => { if (!visible) closeForm() }">
            <div v-if="form === 'node' && newNodeToken" class="workspace-form">
                <p>节点注册码仅显示一次。启动节点时使用此注册码。</p>
                <code class="workspace-token">{{ newNodeToken }}</code>
                <p v-if="formError" role="alert" class="workspace-error">{{ formError }}</p>
                <div class="dialog-actions"><Button label="关闭" outlined :disabled="saving" @click="closeForm" />
                    <Button :label="copiedToken ? '已复制' : '复制注册码'" icon="pi pi-copy" @click="copyToken" /></div>
            </div>
            <form v-else class="workspace-form" @submit.prevent="submitForm">
                <template v-if="form === 'bind'">
                    <label for="bind-code">设备编号</label>
                    <InputText v-if="admin" id="bind-code" v-model="associateId" required autofocus />
                    <InputText v-else id="bind-code" v-model="claimId" placeholder="八位设备编号"
                        autocapitalize="none" :spellcheck="false" required autofocus />
                    <template v-if="admin">
                        <label for="bind-owner">{{ t('controller.owner') }}</label>
                        <Select v-model="bindingOwner" input-id="bind-owner" :aria-label="t('controller.owner')"
                            :options="overview?.users" option-label="username" option-value="id" :placeholder="t('controller.owner')" />
                    </template>
                    <small v-else>在客户端的「设备与控制端」查看或复制设备编号。</small>
                </template>
                <template v-else-if="form === 'network'">
                    <label for="network-name">{{ t('controller.network_name') }}</label>
                    <InputText id="network-name" v-model="newNetwork" :disabled="saving || Boolean(startNetworkId)" required autofocus />
                    <template v-if="admin">
                        <label for="network-owner">{{ t('controller.owner') }}</label>
                        <Select v-model="owner" input-id="network-owner" :aria-label="t('controller.owner')"
                            :options="overview?.users" option-label="username" option-value="id" :placeholder="t('controller.owner')" />
                    </template>
                    <template v-else>
                        <p v-if="startNetworkId">网络已创建。可以继续添加设备，或直接查看网络。</p>
                        <label v-else class="network-device-option">
                            <input v-model="startWithDevices" type="checkbox" :disabled="saving" />
                            同时添加设备
                        </label>
                        <template v-if="startWithDevices">
                            <fieldset v-if="devices.length" class="start-device-group">
                                <legend>已绑定设备</legend>
                                <div class="start-device-list">
                                    <label v-for="device in devices" :key="device.device_id" class="start-device-option">
                                        <input v-model="startDeviceCodes" type="checkbox" :value="device.device_code"
                                            :disabled="saving || startAddedCodes.includes(device.device_code)" />
                                        <span><strong>{{ device.hostname || '未命名设备' }}</strong><code>{{ device.device_code }}</code></span>
                                        <small v-if="startAddedCodes.includes(device.device_code)">已添加</small>
                                        <small v-else-if="!device.online">离线</small>
                                    </label>
                                </div>
                            </fieldset>
                            <label for="start-device-code">新设备编号</label>
                            <InputText id="start-device-code" v-model="startCode" placeholder="八位设备编号"
                                autocomplete="off" autocapitalize="none" :spellcheck="false"
                                :invalid="Boolean(normalizedStartCode) && !deviceCodePattern.test(normalizedStartCode)"
                                :disabled="saving || startAddedCodes.includes(normalizedStartCode)" />
                        </template>
                    </template>
                </template>
                <template v-else-if="form === 'join-network'">
                    <label for="join-network">选择网络</label>
                    <Select v-model="joinNetworkId" input-id="join-network" aria-label="选择网络"
                        :options="networks" option-label="display_name" option-value="network_id" filter placeholder="选择网络" />
                </template>
                <template v-else-if="form === 'node'">
                    <label for="node-name">{{ t('controller.node_name') }}</label>
                    <InputText id="node-name" v-model="nodeName" required autofocus />
                    <label for="node-address">{{ t('controller.node_endpoint') }}</label>
                    <InputText id="node-address" v-model="nodeEndpoint" placeholder="tcp://203.0.113.10:22021" required />
                </template>
                <template v-else-if="form === 'member' && admin">
                    <label for="member-device">{{ t('controller.select_device') }}</label>
                    <Select input-id="member-device" :aria-label="t('controller.select_device')" :options="deviceOptions" option-label="label" option-value="value"
                        filter show-clear :placeholder="t('controller.select_device')" @update:model-value="value => addId = value ?? ''" />
                    <label for="member-code">设备编号</label>
                    <InputText id="member-code" v-model="addId" required />
                </template>
                <template v-else-if="form === 'member'">
                    <fieldset v-if="deviceOptions.length" class="member-device-mode">
                        <legend class="sr-only">添加方式</legend>
                        <label>
                            <input v-model="memberMode" type="radio" name="member-addition-mode" value="existing" :disabled="saving" />
                            已绑定设备
                        </label>
                        <label>
                            <input v-model="memberMode" type="radio" name="member-addition-mode" value="code" :disabled="saving" />
                            新设备编号
                        </label>
                    </fieldset>
                    <template v-if="!useMemberCode">
                        <label for="member-owned-device">{{ t('controller.select_device') }}</label>
                        <Select v-model="addId" input-id="member-owned-device" :aria-label="t('controller.select_device')"
                            :options="deviceOptions" option-label="label" option-value="value" filter
                            :placeholder="t('controller.select_device')" :disabled="saving" />
                    </template>
                    <template v-else>
                        <label for="member-new-code">设备编号</label>
                        <InputText id="member-new-code" v-model="memberCode" placeholder="八位设备编号"
                            autocapitalize="none" :spellcheck="false" :disabled="saving" required />
                    </template>
                </template>
                <template v-else-if="form === 'password'">
                    <label for="new-password">{{ t('web.settings.new_password') }}</label>
                    <Password input-id="new-password" v-model="newPassword" autocomplete="new-password" required toggle-mask :feedback="false" />
                </template>
                <template v-else-if="form === 'upstream-subnet' && upstreamDevice">
                    <p>{{ upstreamDevice.hostname || '未命名设备' }} · <code>{{ upstreamDevice.device_code }}</code></p>
                    <label for="upstream-subnet">{{ t('controller.upstream_subnet') }}</label>
                    <InputText id="upstream-subnet" v-model="upstreamSubnet" placeholder="10.168.10.0/24"
                        aria-describedby="upstream-subnet-help" :invalid="!!formError" :disabled="saving"
                        autocomplete="off" spellcheck="false" maxlength="18" autofocus @update:model-value="formError = ''" />
                    <small id="upstream-subnet-help">{{ t('controller.upstream_subnet_help') }}</small>
                </template>
                <template v-else-if="form === 'rename-device' && deviceToRename">
                    <p>{{ t('controller.device_id') }} · <code>{{ deviceToRename.device_code }}</code></p>
                    <label for="rename-device-name">{{ t('controller.device_name') }}</label>
                    <InputText id="rename-device-name" v-model="deviceName" :invalid="!!formError" :disabled="saving"
                        autocomplete="off" required autofocus @update:model-value="formError = ''" />
                </template>
                <p v-if="formError" role="alert" class="workspace-error">{{ formError }}</p>
                <div class="dialog-actions">
                    <Button v-if="form === 'network' && startNetworkId" type="button" label="查看网络" outlined
                        :disabled="saving" @click="openStartedNetwork(startNetworkId)" />
                    <Button v-else type="button" :label="t('web.common.cancel')" outlined :disabled="saving" @click="closeForm" />
                    <Button v-if="form" type="submit" :label="formSubmitLabel"
                        :disabled="!formValid" :loading="saving" />
                </div>
            </form>
        </Dialog>

        <Dialog :visible="Boolean(confirmation)" modal :header="confirmation?.label" class="workspace-dialog"
            :closable="!saving" :close-on-escape="!saving" @update:visible="visible => { if (!visible) confirmation = undefined }">
            <p>{{ confirmation?.message }}</p>
            <p v-if="confirmError" role="alert" class="workspace-error">{{ confirmError }}</p>
            <div class="dialog-actions">
                <Button type="button" :label="t('web.common.cancel')" outlined :disabled="saving" @click="confirmation = undefined" />
                <Button type="button" :label="confirmation?.label" :loading="saving"
                    :severity="confirmation?.destructive ? 'danger' : undefined" @click="confirmAction" />
            </div>
        </Dialog>
    </div>
</template>
