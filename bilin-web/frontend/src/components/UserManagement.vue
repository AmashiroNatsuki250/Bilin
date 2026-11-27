<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { TieredMenu } from 'primevue'
import type { MenuItem } from 'primevue/menuitem'
import type { ForcedProtocol, PlatformUser, UserPolicyPatch } from '../modules/api'

const props = defineProps<{
    users: PlatformUser[] | null
    currentUser: string
    loading: boolean
    saving: boolean
}>()
const emit = defineEmits<{
    updatePolicy: [user: PlatformUser, policy: UserPolicyPatch]
    deleteUser: [user: PlatformUser]
}>()

const protocols = {
    udp: 'UDP',
    kcp: 'KCP',
    quic: 'QUIC',
    tcp: 'TCP',
    wss: 'WSS',
} satisfies Record<Exclude<ForcedProtocol, 'auto'>, string>
const protocolOptions = Object.entries(protocols) as [keyof typeof protocols, string][]

function overrideLabel(user: PlatformUser) {
    if (user.force_relay) return '强制中转'
    const protocol = user.forced_protocol ?? 'auto'
    if (protocol === 'auto') return ''
    if (!Object.prototype.hasOwnProperty.call(protocols, protocol)) return '强制协议已不可用'
    return `强制协议 · ${protocols[protocol as keyof typeof protocols]}`
}

function userActions(user: PlatformUser): MenuItem[] {
    const current = user.username === props.currentUser
    const locked = !!overrideLabel(user)
    return [
        {
            label: '强制中转', visible: !locked,
            command: () => emit('updatePolicy', user, { force_relay: true }),
        },
        {
            label: '强制协议', visible: !locked,
            items: protocolOptions.map(([protocol, label]) => ({
                label, command: () => emit('updatePolicy', user, { forced_protocol: protocol }),
            })),
        },
        { separator: true, visible: !locked },
        {
            label: user.banned ? '解封' : '封号', disabled: current,
            command: () => emit('updatePolicy', user, { banned: !user.banned }),
        },
        {
            label: '删除用户', class: 'user-actions-danger', disabled: current || user.username === 'admin',
            command: () => emit('deleteUser', user),
        },
    ]
}

type UserActionsMenu = InstanceType<typeof TieredMenu>
const actionMenus = new Map<number, UserActionsMenu>()
const expandedUser = ref<number>()
const actionMenuId = (user: PlatformUser) => `user-actions-${user.id}`
const actionMenuLabel = (user: PlatformUser) => `${user.username} 的更多操作`

function closeMenuFor(userId: number) {
    if (expandedUser.value === userId) expandedUser.value = undefined
}

function setActionMenu(userId: number, menu: UserActionsMenu | null) {
    if (menu) actionMenus.set(userId, menu)
    else {
        actionMenus.delete(userId)
        closeMenuFor(userId)
    }
}

const keyword = ref('')
const page = ref(1)
const pageSize = 10
const results = computed(() => {
    const query = keyword.value.trim().toLowerCase()
    return (props.users ?? []).filter(user => user.username.toLowerCase().includes(query))
})
const pageCount = computed(() => Math.max(1, Math.ceil(results.value.length / pageSize)))
const visibleUsers = computed(() => results.value.slice((page.value - 1) * pageSize, page.value * pageSize))

watch(keyword, () => { page.value = 1 })
watch(pageCount, count => { page.value = Math.min(page.value, count) })
</script>

<template>
    <section class="work-card resource-list" aria-label="用户列表" :aria-busy="saving">
        <div class="resource-toolbar">
            <label class="resource-search">
                <i class="pi pi-search" aria-hidden="true" />
                <input v-model="keyword" type="search" placeholder="搜索用户名" aria-label="搜索用户名" />
            </label>
        </div>

        <div v-if="visibleUsers.length" class="work-table-scroll" tabindex="0" role="region" aria-label="用户策略">
            <table class="work-table user-policy-table">
                <thead>
                    <tr>
                        <th scope="col">用户</th>
                        <th scope="col">账号状态</th>
                        <th scope="col">连接策略</th>
                        <th scope="col">操作</th>
                    </tr>
                </thead>
                <tbody>
                    <tr v-for="user in visibleUsers" :key="user.id">
                        <th scope="row">
                            <span class="table-entity">
                                <span class="work-icon"><i class="pi pi-user" aria-hidden="true" /></span>
                                <span>
                                    <strong>{{ user.username }}</strong>
                                    <small v-if="user.username === currentUser" class="work-muted">当前账号</small>
                                </span>
                            </span>
                        </th>
                        <td><span class="work-chip" :class="{ 'chip-online': !user.banned }">{{ user.banned ? '已封禁' : '正常' }}</span></td>
                        <td>
                            <div class="policy-actions">
                                <template v-if="overrideLabel(user)">
                                    <span class="work-chip">{{ overrideLabel(user) }}</span>
                                    <button type="button" class="policy-action" :disabled="saving"
                                        @click="emit('updatePolicy', user, { force_relay: false, forced_protocol: 'auto' })">
                                        恢复默认
                                    </button>
                                </template>
                                <template v-else>
                                    <label class="policy-preference">
                                        <input type="checkbox" :checked="user.udp_enabled" :disabled="saving"
                                            :aria-label="`${user.username} 的智能协议`" @click.prevent="emit('updatePolicy', user, { udp_enabled: !user.udp_enabled })" />
                                        <span>{{ user.udp_enabled ? '智能协议' : '智能协议（默认 TCP）' }}</span>
                                    </label>
                                </template>
                            </div>
                        </td>
                        <td>
                            <button type="button" class="policy-action policy-more" :disabled="saving"
                                :aria-label="actionMenuLabel(user)" aria-haspopup="menu"
                                :aria-controls="expandedUser === user.id ? actionMenuId(user) : undefined"
                                :aria-expanded="expandedUser === user.id"
                                @click="actionMenus.get(user.id)?.toggle($event)">
                                更多 <i class="pi pi-chevron-down" aria-hidden="true" />
                            </button>
                            <TieredMenu :ref="instance => setActionMenu(user.id, instance as UserActionsMenu | null)"
                                :id="actionMenuId(user)" :aria-label="actionMenuLabel(user)" :model="userActions(user)" popup
                                :disabled="saving" @before-show="expandedUser = user.id" @before-hide="closeMenuFor(user.id)" />
                        </td>
                    </tr>
                </tbody>
            </table>
        </div>
        <div v-else-if="users || loading" class="work-empty">
            <span class="work-icon"><i class="pi pi-users" aria-hidden="true" /></span>
            <p>{{ users ? (keyword.trim() ? '没有匹配的用户' : '暂无用户') : '正在读取用户' }}</p>
        </div>

        <footer v-if="results.length" class="resource-footer">
            <span>共 {{ results.length }} 个用户</span>
            <div v-if="pageCount > 1" class="resource-pagination">
                <button type="button" aria-label="上一页" :disabled="page === 1" @click="page--"><i class="pi pi-chevron-left" aria-hidden="true" /></button>
                <span>第 {{ page }} / {{ pageCount }} 页</span>
                <button type="button" aria-label="下一页" :disabled="page === pageCount" @click="page++"><i class="pi pi-chevron-right" aria-hidden="true" /></button>
            </div>
        </footer>
    </section>
</template>

<style scoped>
.user-policy-table { min-width: 620px; }
.policy-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
.policy-preference {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 0;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 570;
    color: #525252;
}
.policy-preference input { flex: none; width: 14px; height: 14px; accent-color: #242424; }
.policy-action {
    padding: 4px 11px;
    border: 1px solid #d3d3d3;
    border-radius: 6px;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 570;
}
.policy-action:hover:enabled { border-color: #949494; background: #f4f4f4; }
.policy-more { display: inline-flex; align-items: center; gap: 7px; min-height: 32px; }
.policy-more .pi { font-size: 10px; }
:global(.user-actions-danger .p-tieredmenu-item-label) { color: #b42318; }
.policy-action:disabled { opacity: .48; }
.policy-preference input:focus-visible, .policy-action:focus-visible { outline: 2px solid #676767; outline-offset: 3px; }
</style>
